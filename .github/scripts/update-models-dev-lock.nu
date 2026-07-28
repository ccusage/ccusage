#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell nixpkgs#git --command nu

const SNAPSHOTS = [
    'rust/crates/ccusage-core/src/models-dev-pricing.json'
    'rust/adapters/codex/src/codex-auto-review-fallbacks.json'
]

# Bump the pinned models.dev input, regenerate the committed snapshots, and report
# whether anything the build embeds actually moved.
def main [] {
    # A snapshot that moved in the Rust tree would otherwise only surface as a
    # `git checkout` pathspec failure after all the regeneration work, which is
    # how this workflow stayed broken for weeks after the crate split.
    let missing = $SNAPSHOTS | where {|path| not ($path | path exists) }
    if ($missing | is-not-empty) {
        error make {msg: $"snapshot paths no longer exist: ($missing | str join ', ')"}
    }

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

    report {
        changed: $changed
        paths: ($SNAPSHOTS | prepend flake.lock)
    }
}

# Whether git reports tracked changes for the given paths.
def dirty [...paths: string]: nothing -> bool {
    (^git diff --quiet -- ...$paths | complete).exit_code != 0
}

# Hand the workflow both the verdict and the paths to commit, so the file list
# lives in one place instead of being repeated in the workflow.
def report [result: record]: nothing -> nothing {
    [
        $"changed=($result.changed)"
        $"paths=($result.paths | str join ' ')"
    ]
    | each {|line| $"($line)(char nl)" | save --append $env.GITHUB_OUTPUT }
    | ignore
}
