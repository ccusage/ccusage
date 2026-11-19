#!/usr/bin/env nix
#! nix shell --inputs-from .. nixpkgs#nushell --command nu

# Report the parse and type errors Nushell finds in every script under a directory, without running any of them.
def main [root: path]: nothing -> nothing {
    cd $root
    let findings = glob "**/*.nu" --exclude ["**/node_modules/**" "**/target/**"]
    | each {|file| diagnostics ($file | path relative-to (pwd)) }
    | flatten
    | sort-by file line column

    if ($findings | is-not-empty) {
        $findings
        | each {|finding| print --stderr $"($finding.file):($finding.line):($finding.column): ($finding.severity): ($finding.message)" }
        | ignore
        error make --unspanned {msg: $"Nushell reported ($findings | length) diagnostics."}
    }
}

def diagnostics [file: string]: nothing -> list<record<file: string, line: int, column: int, severity: string, message: string>> {
    let result = run-external $nu.current-exe '--no-config-file' '--ide-check' '100' $file | complete
    if $result.exit_code != 0 {
        error make --unspanned {msg: $"nu --ide-check failed on ($file): ($result.stderr)"}
    }
    let source = open --raw $file | into binary
    $result.stdout
    | lines
    | each { from json }
    | where type == diagnostic
    | uniq
    | each {|diagnostic|
        let preceding = $source | bytes at ..<($diagnostic.span.start) | decode utf-8 | split row "\n"
        {
            file: $file
            line: ($preceding | length)
            column: (($preceding | last | str length) + 1)
            severity: $diagnostic.severity
            message: $diagnostic.message
        }
    }
}
