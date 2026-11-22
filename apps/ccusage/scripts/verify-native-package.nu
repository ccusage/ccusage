#!/usr/bin/env nix
#! nix shell --inputs-from ../../.. nixpkgs#nushell --command nu

use ./native-package-telemetry.nu verify_telemetry_endpoint

def main []: nothing -> nothing {
    let binary_path = (match (configured_binary_path) {
        null => (error make {msg: 'Native package binary is not configured'})
        $path => $path
    })
    let resolved = $binary_path | path expand
    match (binary_issue $binary_path $resolved) {
        null => null
        $issue => (error make {msg: $"Native package binary is not ready: ($issue)"})
    }
    let telemetry_enabled = ($env.CCUSAGE_TELEMETRY_ENABLED? | default 'false') == 'true'
    verify_telemetry_endpoint $resolved $telemetry_enabled
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
