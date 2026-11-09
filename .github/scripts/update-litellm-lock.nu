#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell nixpkgs#git --command nu

use ./pricing-lock.nu report

# Bump the pinned LiteLLM input and report whether it moved the pricing JSON that
# the build embeds, so the workflow can skip validating lock-only churn.
#
# The input is a single-file URL (not the 2.7 GB repository), and that URL embeds
# the commit, so `nix flake update` alone cannot move it: rewrite the commit in
# flake.nix first, then let Nix re-lock the new narHash.
def main [] {
    let before = (locked-pricing)
    let rev = (
        ^git ls-remote https://github.com/BerriAI/litellm refs/heads/main
        | split row "\t"
        | first
    )
    open --raw flake.nix
    | str replace --regex 'BerriAI/litellm/[0-9a-f]{40}/' $"BerriAI/litellm/($rev)/"
    | save --force flake.nix
    ^nix flake update litellm

    let changed = match ((locked-pricing) == $before) {
        false => true
        true => {
            print 'LiteLLM pricing JSON is unchanged; dropping the lock-only bump.'
            ^git checkout -- flake.nix flake.lock
            false
        }
    }

    report {
        changed: $changed
        paths: [flake.nix flake.lock]
    }
}

# Fetch the pricing JSON from the URL flake.lock pins.
def locked-pricing []: nothing -> record {
    http get (open --raw flake.lock | from json | get nodes.litellm.locked.url)
}
