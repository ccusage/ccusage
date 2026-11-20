#!/usr/bin/env nix
#! nix shell --inputs-from .. nixpkgs#nushell --command nu

const exempt_crate = 'ccusage_zstd'

const wrapper_body = '
def --wrapped main [rustc: string, ...args: string]: nothing -> nothing {
    let crate = $args | window 2 | where {|pair| $pair.0 == "--crate-name" } | get --optional 0.1
    let forbid = if $crate == "EXEMPT" { [] } else { ["-F" "unsafe_code"] }
    run-external $rustc ...$args ...$forbid
}
'

# Checks every workspace crate except ccusage-zstd with unsafe_code forbidden on the rustc command line.
def main [manifest: path, --target-dir: path]: nothing -> nothing {
    let target = $target_dir | default (
        $manifest
        | path expand
        | path dirname
        | path join target unsafe-code
    )
    mkdir $target
    let wrapper = $target | path join forbid-unsafe-rustc
    $"#!($nu.current-exe) --no-config-file\n($wrapper_body | str replace 'EXEMPT' $exempt_crate)"
    | save --force $wrapper
    ^chmod +x $wrapper
    with-env {RUSTC_WORKSPACE_WRAPPER: $wrapper} {
        (
            ^cargo check
            --manifest-path $manifest
            --workspace
            --all-targets
            --locked
            --target-dir $target
        )
    }
}
