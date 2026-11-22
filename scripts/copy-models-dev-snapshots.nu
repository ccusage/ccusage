#!/usr/bin/env nix
#! nix shell --inputs-from .. nixpkgs#nushell nixpkgs#nix nixpkgs#git --command nu

export def snapshot-paths [output: path]: nothing -> list<string> {
    let paths = glob ($output | path join '**/*.json')
    | each {|file| $file | path relative-to $output }
    | sort
    if ($paths | is-empty) {
        error make {msg: 'The pricing generator produced no snapshots'}
    }
    $paths
}

export def build-snapshots []: nothing -> string {
    let build = ^nix build .#models-dev-pricing --no-link --print-out-paths | complete
    if $build.exit_code != 0 {
        error make {msg: $build.stderr}
    }
    $build.stdout | str trim
}

def main []: nothing -> nothing {
    let output = build-snapshots
    let paths = snapshot-paths $output
    $paths | each {|path|
        open --raw ($output | path join $path) | save --force $path
        ^chmod u+w $path
    } | ignore
    ^nix fmt ...$paths
}
