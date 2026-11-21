use lex.nu lexical-comments

const config = path self sgconfig.yml
const bash_rule = path self rules/bash.yml
const date = '[0-9]{4}-[0-9]{2}-[0-9]{2}'
const note = '^NOTE\((?<date>' + $date + ')(?:, (?<cli>[^,#]+ v?[0-9]+(?:\.[0-9]+)+(?:[-+][0-9A-Za-z.-]+)?))?(?:, #[1-9][0-9]*)?\): (?<body>\S.*)$'
const marker = '^(?://|#|;+) '
const github_action_sha_pin = r#'^(?:-\s+)?uses:\s+(["']?)[A-Za-z0-9][A-Za-z0-9-]*/[\w.-]+(?:/[\w.-]+)*@[[:xdigit:]]{40}\1$'#
const renovate_version = '^# v?[0-9]+(?:\.[0-9]+){0,2}(?:[-+][0-9A-Za-z.-]+)?$'
const work_marker = '\b(?:TODO|FIXME|HACK|XXX)\b'
const nu_def = '^\s*(?:export\s+)?def(?:\s+--[\w-]+)*\s+'
const fixture_path = '(?:^|/)(?:fixtures|snapshots)/|\.(?:snap|snap\.new|golden)$'
const generated_header = '(?i)^\s*(?://|/\*+|\*|#|;+)\s*(?:@generated\b|(?:auto[- ]?)?generated (?:by|from|automatically)\b|do not edit\b)'
const license_header = '(?i)^(?://+|/\*+|#+|;+)[\s*]*(?:SPDX-License-Identifier:|Copyright\b|Licensed under\b)'
const format_words = '(?i)\b(?:log|rollout|session|record|event)(?: file)? (?:format|schema)\b'
const agent_words = '(?i)\b(?:agent|claude|codex|opencode|copilot|openclaw|cursor|gemini|kilo|kimi|pi cli)\b'
const shell_extensions = ['sh' 'bash' 'zsh']
const nix_interpreter = '^#!(?:\S*/nix|\S*/env[ \t]+(?:-S[ \t]+)?nix)[ \t]*$'
const nix_shebang_block = '^/\*\n(?:#![ \t]+nix[ \t]+\S[^\n]*\n)+[ \t]*\*/$'

const script_extensions = [
    'js'
    'jsx'
    'mjs'
    'cjs'
    'ts'
    'tsx'
    'mts'
    'cts'
]

def language [file: string]: nothing -> string {
    let parts = $file | path parse
    match $parts.extension {
        'rs' => 'rust'
        'nix' => 'nix'
        'nu' => 'nu'
        'toml' => 'toml'
        'yaml' | 'yml' => 'yaml'
        'bb' | 'clj' | 'cljs' | 'cljc' => 'clojure'
        'jsonc' => 'json'
        $ext if $ext in $shell_extensions => 'bash'
        $ext if $ext in $script_extensions => 'script'
        _ => {
            match ($parts.stem | str lowercase) {
                'justfile' | '.justfile' => 'just'
                '.envrc' => 'bash'
                '.gitignore' | '.gitattributes' | '.tagpr' | '.dockerignore' => 'config'
                _ => ''
            }
        }
    }
}

def valid-date [value: string]: nothing -> bool {
    try { ($value | into datetime --format '%Y-%m-%d') != null } catch { false }
}

def rust-target [file: string]: nothing -> string {
    let parts = $file | path split
    let default_root = '^(?:src/main\.rs|src/bin/(?:[^/]+\.rs|[^/]+/main\.rs)|(?:tests|examples|benches)/(?:[^/]+\.rs|[^/]+/main\.rs)|build\.rs)$'
    for count in ((($parts | length) - 1)..0) {
        let root = if $count == 0 {
            '.'
        } else {
            $parts | take $count | path join
        }
        let manifest = $root | path join Cargo.toml
        if not ($manifest | path exists) { continue }
        let cargo = open $manifest
        let relative = $file | path relative-to $root
        let targets = ['bin' 'test' 'example' 'bench'] | each {|kind|
            $cargo | get -o $kind | default [] | get -o path | default []
        } | flatten
        if $relative == ($cargo | get -o lib.path | default 'src/lib.rs') {
            return 'library-root'
        }
        if $relative in $targets or $relative =~ $default_root { return 'root' }
        if (($root | path join ($cargo | get -o lib.path | default 'src/lib.rs')) | path exists) and ($relative =~ '^src/(?!bin/)') {
            return 'library-module'
        }
        return ''
    }
    ''
}

def syntax [files: list<string>]: nothing -> list<record> {
    if ($files | is-empty) { return [] }
    let result = ^ast-grep scan --config $config --no-ignore hidden --json=compact -- $files.0 ...($files | skip 1) | complete
    if $result.exit_code != 0 and not ($result.exit_code == 1 and ($result.stderr | is-empty)) {
        error make {msg: $"ast-grep failed: ($result.stderr)"}
    }
    if ($result.stdout | str trim | is-empty) {
        []
    } else {
        $result.stdout | from json
    }
}

def prefix [line: string, column: int]: nothing -> string {
    $line | str substring (0..<$column) | str trim
}

def ast-comments [matches: list<record>, language: string]: nothing -> list<record> {
    $matches | where {|item| $item.ruleId | str ends-with '-comment' } | each {|item|
        $item.range.start | merge {text: ($item.text | str trim --right), language: $language}
    }
}

def embedded-comments [node: record, lines: list<string>]: nothing -> list<record> {
    let start = $node.range.start.line
    let nix = $node.ruleId == 'nix-shell-script'
    let body = if $nix {
        $node.text | str substring 2..-3 | lines
    } else {
        [''] ++ ($node.text | lines | skip 1)
    }
    let content = $body | where {|line| not ($line | str trim | is-empty) }
    if ($content | is-empty) { return [] }
    let indent = $content | each {|line|
        ($line | str length) - ($line | str trim --left | str length)
    } | math min
    let script = $body | str substring $indent.. | str join (char nl)
    let result = $script | ^ast-grep scan --rule $bash_rule --stdin --json=compact | complete
    if $result.exit_code != 0 and not ($result.exit_code == 1 and ($result.stderr | is-empty)) {
        error make {msg: $"embedded shell scan failed: ($result.stderr)"}
    }
    if ($result.stdout | str trim | is-empty) { return [] }
    ast-comments ($result.stdout | from json) 'bash' | each {|comment|
        let line = $start + $comment.line
        let original = $lines | get $line
        let marker_start = $original | str index-of --end $comment.text
        if $marker_start == -1 { return null }
        $comment | merge {line: $line, column: $marker_start}
    }
}

def just-docs [file: string, lines: list<string>]: nothing -> list<int> {
    let dump = ^just --justfile $file --dump --dump-format json | complete
    if $dump.exit_code != 0 { error make {msg: $dump.stderr} }
    let recipes = $dump.stdout | from json | get recipes
    $lines | enumerate | where {|row| $row.item =~ '^\s*#' } | where {|row|
        let declaration = $lines | skip ($row.index + 1) | skip while {|line| $line =~ '^\s*\[' } | first | default ''
        let name = $declaration | parse --regex '^@?(?<name>[A-Za-z_][\w-]*)(?:\s+[^:]*)?\s*:(?:[^=]|$)' | get -o 0.name
        if $name == null { return false }
        let doc = $recipes | get -o $name | get -o doc
        $doc != null and $doc == ($row.item | str replace --regex '^\s*#\s*' '' | str trim)
    } | get index
}

def nu-help [lines: list<string>, comments: list<record>]: nothing -> list<int> {
    $lines | enumerate | where {|row| $row.item =~ $nu_def } | each {|row|
        let help = $lines | take $row.index | reverse | take while {|line| $line =~ '^\s*#' } | length
        let first = $row.index - $help
        let parameters = if $row.item =~ '\[[^\]]*$' {
            $lines | skip ($row.index + 1) | take while {|line| $line !~ '^\s*\]' } | length
        } else { 0 }
        let end = $row.index + $parameters
        $comments | where {|comment|
            (($comment.line >= $first and $comment.line < $row.index)
                or ($comment.line > $row.index and $comment.line <= $end
                    and (prefix ($lines | get $comment.line) $comment.column) =~ '^(?:\.\.\.)?(?:--?)?[\w-]+(?:\([^)]+\))?\??\s*[:=,]'))
        } | get line
    } | flatten
}

def header-lines [lines: list<string>, comments: list<record>]: nothing -> list<int> {
    let header = $comments | take while {|comment|
        not ($lines | take $comment.line | any {|line| $line !~ '^\s*(?://|#|;|/\*|\*|$)' })
    }
    $header | enumerate | where {|row|
        let previous = $header | take ($row.index + 1) | reverse
        let first = $previous | enumerate | take while {|item|
            $item.item.line >= ($row.item.line - $item.index)
        } | get item | last
        $row.item.text =~ $license_header or $first.text =~ $license_header
    } | get item.line
}

def directive [text: string, language: string]: nothing -> bool {
    if $text =~ '^#!(?:\s|/)' { return true }
    match $language {
        'bash' | 'just' => ($text =~ '^#\s*shellcheck\s+[a-z_]+=')
        'yaml' => ($text =~ '^#\s*yaml-language-server:\s*\$schema=')
        'script' => {
            (($text =~ '^(?://|/\*)\s*(?:biome-ignore(?:-all|-start|-end)?\s+\S|(?:oxlint|eslint)-(?:disable|enable)(?:-next-line|-line)?(?:\s|$)|prettier-ignore\b|@ts-(?:no)?check\b)')
                or ($text =~ '(?s)^/\*\*\s*\*?\s*@(?:type|typedef|param|returns|template|import|satisfies)\s' and
                    ($text | lines | all {|line| $line =~ '^\s*(?:/\*\*\s*|\*\s*)?(?:@(?:type|typedef|param|returns|template|import|satisfies)\s|\*/\s*$|$)' })))
        }
        _ => false
    }
}

def dated-directive [text: string]: nothing -> bool {
    let reason = $text | str replace --regex '(?s)^(?://|/\*)\s*@ts-expect-error\b' ''
    let dates = $reason | parse --regex ('\b(?<date>' + $date + ')\b')
    ($dates | any {|item| valid-date $item.date }) and ($reason | str replace --all --regex $date '') =~ '[[:alpha:]]'
}

def violations [
    file: string
    lang: string
    lines: list<string>
    raw: list<record>
]: nothing -> list<record> {
    let comments = if $lang in ['nu' 'toml' 'just' 'clojure' 'config'] {
        lexical-comments $lines $lang | insert language $lang
    } else {
        let embedded = $raw | where {|item| $item.ruleId | str ends-with '-script' } | each {|item|
            embedded-comments $item $lines
        } | flatten
        (ast-comments $raw $lang) ++ $embedded
    }
    let comments = $comments | sort-by line column
    if ($comments | is-empty) { return [] }
    if ($comments | take 5 | any {|comment| $comment.line < 6 and $comment.text =~ $generated_header }) { return [] }
    let helpers = match $lang {
        'just' => (just-docs $file $lines)
        'nu' => (nu-help $lines $comments)
        _ => []
    }
    let licenses = header-lines $lines $comments
    let target = if $lang == 'rust' and ($raw | any {|item| $item.ruleId =~ '^rust-(?:inner|public)-doc$' }) {
        rust-target $file
    } else { '' }
    let rust_locations = $raw | where {|item| not ($item.ruleId | str ends-with '-comment') }
    $comments | each {|comment|
        let text = $comment.text | str trim --right
        let line = $comment.line
        let before = prefix ($lines | get $line) $comment.column
        if $line in $licenses or (directive $text $comment.language) or ($lang == 'nu' and $line in $helpers) { return [] }
        if ($comment.language == 'yaml' and $file =~ '^\.github/.*\.ya?ml$'
            and $before =~ $github_action_sha_pin and $text =~ $renovate_version) {
            return []
        }
        if ($lang == 'script' and $line == 1 and $comment.column == 0
            and $lines.0 =~ $nix_interpreter and $text =~ $nix_shebang_block) {
            return []
        }
        let doc = $rust_locations | where {|item| $item.range.start.line == $line }
        let rules = [
            (if $before != '' { 'trailing-comment' })
            (if $text =~ $work_marker { 'work-marker' })
            (if $lang == 'script' and $text =~ '^(?://|/\*)\s*@ts-expect-error\b' {
                if not (dated-directive $text) { 'dated-directive' }
            } else if $lang == 'rust' and $text =~ '^// SAFETY:' {
                if $text !~ '^// SAFETY: \S' or not ($rust_locations | any {|item|
                    $item.ruleId == 'rust-unsafe' and $item.range.start.line == ($line + 1)
                }) { 'safety-placement' }
            } else if $lang == 'rust' and $text =~ '^///(?:[^/]|$)|^//!|^#!?\[\s*doc\s*=' {
                let kinds = $doc | get ruleId
                if not ('rust-schema-doc' in $kinds
                    or ('rust-public-doc' in $kinds and $target in ['library-module' 'library-root'])
                    or ('rust-inner-doc' in $kinds and $target in ['root' 'library-root'])) { 'rust-doc-scope' }
            } else if $lang == 'just' and $line in $helpers {
                null
            } else {
                let parsed = $text | str replace --regex $marker '' | parse --regex $note | first
                if $parsed == null or not (valid-date $parsed.date) or $parsed.body !~ '\S\. \S.*\.$' {
                    'note-format'
                } else if ($parsed.cli | is-empty) and $parsed.body =~ $format_words and ($file =~ '(?:^|/)rust/adapters/' or $parsed.body =~ $agent_words) {
                    'agent-cli-version'
                }
            })
        ] | compact
        $rules | each {|rule| {path: $file, line: ($line + 1), rule: $rule, text: ($text | lines | first)} }
    } | flatten
}

export def scan-comments [files: list<string>, --include-fixtures]: nothing -> list<record> {
    let sources = $files | where {|file|
        (($file | path type) == 'file' and ($include_fixtures or $file !~ $fixture_path)
            and $file != 'pnpm-lock.yaml')
    } | each {|file| {file: $file, language: (language $file)} } | where language != ''
    let structural = $sources | where {|source|
        $source.language not-in ['nu' 'toml' 'just' 'clojure' 'config']
    } | get file | chunks 128 | each {|batch| syntax $batch } | flatten | group-by file
    $sources | each {|source|
        violations $source.file $source.language (open --raw $source.file | lines) ($structural | get -o $source.file | default [])
    } | flatten | sort-by path line rule
}

def source-files []: nothing -> list<string> {
    glob --no-dir '**/*' | each {|file| $file | path relative-to $env.PWD }
}

def scan-ref [ref: string]: nothing -> list<record> {
    let work = mktemp -d
    let archive = $work | path join source.tar
    let findings = try {
        ^git archive --format=tar --output $archive $ref
        ^tar -xf $archive -C $work
        do {
            cd $work
            scan-comments (source-files)
        }
    } catch {|err|
        rm -rf $work
        error make $err
    }
    rm -rf $work
    $findings
}

def main [--report-only, --source-tree, --ref: string]: nothing -> nothing {
    let findings = if $ref != null {
        scan-ref $ref
    } else if $source_tree {
        scan-comments (source-files)
    } else {
        let files = ^git ls-files --deduplicate -z | split row (char nul) | where {|file| $file != '' }
        scan-comments $files
    }
    for finding in $findings {
        print $"($finding.path):($finding.line): ($finding.rule): ($finding.text)"
    }
    if not $report_only and not ($findings | is-empty) { exit 1 }
}
