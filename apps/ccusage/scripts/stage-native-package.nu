#!/usr/bin/env nix
#! nix shell --inputs-from ../../.. nixpkgs#nushell --command nu

use ./native-package-telemetry.nu verify_telemetry_endpoint

def main [--platform: string, --arch: string, --binary: string]: nothing -> nothing {
    if $platform == 'win32' {
        error make {
            msg: 'Windows packages are staged by .github/actions/build-windows-native-package, which runs without Nix'
        }
    }
    let repo_root = $env.CURRENT_FILE | path dirname | path join ../../.. | path expand
    let package_root = $repo_root | path join packages $"ccusage-($platform)-($arch)"
    let manifest_path = $package_root | path join package.json
    if not ($manifest_path | path exists) {
        error make {msg: $"Unsupported native package target: ($platform)-($arch)"}
    }
    let binary_entry = (match (open $manifest_path | get --optional files | default [] | where {|file| $file | str starts-with 'bin/' } | get --optional 0) {
        null => (error make {msg: $"($manifest_path) lists no bin/ file"})
        $entry => $entry
    })
    let target = $package_root | path join $binary_entry
    mkdir ($target | path dirname)
    cp -f ($binary | path expand) $target
    chmod 755 $target
    let telemetry_enabled = ($env.CCUSAGE_TELEMETRY_ENABLED? | default 'false') == 'true'
    verify_telemetry_endpoint $target $telemetry_enabled
    print $target
}
