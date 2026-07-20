#!/usr/bin/env nix
#! nix shell --inputs-from ../../.. nixpkgs#nushell --command nu
def main [] {
    let package_json = (open package.json)
    let binary_path = (
        $package_json.files? | default [] | where {|file| $file | str starts-with 'bin/ccusage' } | first
    )
    if ($binary_path | is-empty) {
        error make {msg: 'Native package binary file is not configured'}
    }
    let resolved_binary_path = $binary_path | path expand
    try {
        if not ($resolved_binary_path | path exists) {
            error make {msg: $"($binary_path) does not exist"}
        }
        let entry = ls $resolved_binary_path | first
        if $entry.type != file {
            error make {msg: $"($binary_path) is not a file"}
        }
        if not ($binary_path | str ends-with '.exe') {
            let executable = run-external test '-x' $resolved_binary_path | complete
            if $executable.exit_code != 0 {
                error make {msg: $"($binary_path) is not executable"}
            }
        }
    } catch {|err| error make {msg: $"Native package binary is not ready: ($err.msg)"} }
}
