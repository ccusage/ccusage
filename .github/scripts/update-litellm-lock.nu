#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell nixpkgs#git --command nu

# Bump the pinned LiteLLM input and report whether it moved the pricing JSON that
# the build embeds, so the workflow can skip validating lock-only churn.
def main [] {
    let before = (locked-pricing)
    ^nix flake update litellm

    let changed = match ((locked-pricing) == $before) {
        false => true
        true => {
            print 'LiteLLM pricing JSON is unchanged; dropping the lock-only bump.'
            ^git checkout -- flake.lock
            false
        }
    }

    report {
        changed: $changed
        paths: [flake.lock]
    }
}

# Fetch model_prices_and_context_window.json at the revision flake.lock pins.
def locked-pricing []: nothing -> record {
    let rev = open --raw flake.lock | from json | get nodes.litellm.locked.rev
    http get $"https://raw.githubusercontent.com/BerriAI/litellm/($rev)/model_prices_and_context_window.json"
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
