#!/usr/bin/env nix
#! nix shell --inputs-from .. nixpkgs#nushell --command nu

# Checks every workspace crate except ccusage-zstd, for all features and targets, with unsafe_code forbidden on the rustc command line.
def main [manifest: path, --target-dir: path, --target: string]: nothing -> nothing {
    let target_dir = $target_dir | default (
        $manifest
        | path expand
        | path dirname
        | path join target unsafe-code
    )
    mkdir $target_dir
    let wrapper = $target_dir | path join (
        if $nu.os-info.name == 'windows' { 'forbid-unsafe-rustc.exe' } else { 'forbid-unsafe-rustc' }
    )
    (
        ^rustc --edition 2024 -O
        ($env.FILE_PWD | path join forbid-unsafe-rustc.rs)
        -o $wrapper
    )
    let platform = if $target == null { [] } else { [--target $target] }
    with-env {RUSTC_WORKSPACE_WRAPPER: $wrapper} {
        (
            ^cargo check
            --manifest-path $manifest
            --workspace
            --all-targets
            --all-features
            --locked
            --target-dir $target_dir
            ...$platform
        )
    }
}
