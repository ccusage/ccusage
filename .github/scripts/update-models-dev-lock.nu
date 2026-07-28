#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell nixpkgs#git --command nu

const SNAPSHOTS = [
    'rust/crates/ccusage-core/src/models-dev-pricing.json'
    'rust/crates/ccusage-adapter-codex/src/codex-auto-review-fallbacks.json'
]

# Bump the pinned models.dev input, regenerate the committed snapshots, and report
# whether anything the build embeds actually moved.
def main [] {
    ^nix flake update models-dev
    ^nix develop --command just gen-models-dev-pricing

    let state = {
        snapshots: (dirty ...$SNAPSHOTS)
        lock: (dirty flake.lock)
    }
    let changed = match $state {
        {snapshots: true} => true
        {snapshots: false, lock: true} => {
            print 'models.dev pricing snapshots are unchanged; dropping the lock-only bump.'
            ^git checkout -- flake.lock ...$SNAPSHOTS
            false
        }
        _ => false
    }

    $"changed=($changed)(char nl)" | save --append $env.GITHUB_OUTPUT
}

# Whether git reports tracked changes for the given paths.
def dirty [...paths: string]: nothing -> bool {
    (^git diff --quiet -- ...$paths | complete).exit_code != 0
}
