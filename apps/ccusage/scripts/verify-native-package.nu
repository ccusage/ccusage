#!/usr/bin/env nix
#! nix shell --inputs-from ../../.. nixpkgs#nushell --command nu
def main [] {
    let binary_path = (match (configured_binary_path) {
        null => (error make {msg: 'Native package binary is not configured'})
        $path => $path
    })
    match (binary_issue $binary_path ($binary_path | path expand)) {
        null => null
        $issue => (error make {msg: $"Native package binary is not ready: ($issue)"})
    }
}
def configured_binary_path [] {
    open package.json
    | get --optional files
    | default []
    | where {|file| $file | str starts-with 'bin/ccusage' }
    | get --optional 0
}
def binary_issue [binary_path: string, resolved: path] {
    if not ($resolved | path exists) {
        return $"($binary_path) does not exist"
    }
    match ($resolved | path type) {
        'file' => (executable_issue $binary_path $resolved)
        _ => $"($binary_path) is not a file"
    }
}
def executable_issue [binary_path: string, resolved: path] {
    if ($binary_path | str ends-with '.exe') {
        return null
    }
    match (run-external test '-x' $resolved | complete | get exit_code) {
        0 => null
        _ => $"($binary_path) is not executable"
    }
}
