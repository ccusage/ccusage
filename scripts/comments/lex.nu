const quote_patterns = {
    nu: "r#+'|'|\"|`|#|[^r'\"`#\\\\]+|\\\\.|."
    toml: "'''|\"\"\"|'|\"|#|[^'\"#\\\\]+|\\\\.|."
    just: "'''|\"\"\"|'|\"|`|#|[^'\"`#\\\\]+|\\\\.|."
    clojure: '"|;|[^";\\]+|\\.|.'
    config: '#|[^#]+'
}

export def lexical-comments [lines: list<string>, language: string]: nothing -> list<record> {
    let pattern = '(?<token>' + ($quote_patterns | get $language) + ')'
    let marker = if $language == 'clojure' { ';' } else { '#' }
    mut quote = ''
    mut comments = []
    for row in ($lines | enumerate) {
        mut column = 0
        mut skip_until = 0
        for token in ($row.item | parse --regex $pattern | get token) {
            let width = $token | str length --utf-8-bytes
            let start = $column
            $column = $column + $width
            if $start < $skip_until { continue }
            if $quote != '' {
                if $token == $quote or ($token == "'" and ($quote | str starts-with "'#") and ($row.item | str substring $start.. | str starts-with $quote)) {
                    $skip_until = $start + ($quote | str length)
                    $quote = ''
                }
            } else if $token == $marker {
                if $language in ['nu' 'just'] and $start != 0 and ($row.item | str substring (0..<$start)) !~ '[\s,;|(){}\[\]]$' {
                    continue
                }
                $comments = (
                    $comments
                    | append {line: $row.index, column: $start, text: ($row.item | str substring $start..)}
                )
                break
            } else if $token in ["'" '"' '`' "'''" '"""'] {
                $quote = $token
            } else if $language == 'nu' and ($token =~ "^r#+'$") {
                $quote = "'" + ($token | str replace --regex "^r(#+)'$" '$1')
            }
        }
    }
    $comments
}
