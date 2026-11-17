#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell nixpkgs#git --command nu

use ./pricing-lock.nu [report run-update]
use ../../scripts/copy-models-dev-snapshots.nu [snapshot-paths build-snapshots]

def main [] {
    run-update nix flake update models-dev
    run-update nix shell --inputs-from . nixpkgs#just --command just gen-models-dev-pricing

    let output = build-snapshots
    let snapshots = snapshot-paths ($output | str trim)
    let state = {
        snapshots: (dirty ...$snapshots)
        lock: (dirty flake.lock)
    }
    let changed = match $state {
        {snapshots: true} => true
        {snapshots: false, lock: true} => {
            print --stderr 'models.dev pricing snapshots are unchanged; dropping the lock-only bump.'
            ^git checkout -- flake.lock ...$snapshots
            false
        }
        _ => false
    }

    report {
        changed: $changed
        paths: ($snapshots | prepend flake.lock)
    }
}

# Whether git reports tracked changes for the given paths.
def dirty [...paths: string]: nothing -> bool {
    (^git diff --quiet -- ...$paths | complete).exit_code != 0
}
