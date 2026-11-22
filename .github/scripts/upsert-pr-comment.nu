#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell nixpkgs#gh --command nu

use ./contribution-gate/core.nu [repository required-env]
use ./contribution-gate/mutations.nu upsert-comment

def main []: nothing -> nothing {
    let body = open --raw (required-env COMMENT_FILE)
    let marker = $body | lines | first
    if not ($marker =~ '^<!-- ccusage-perf-comment:[0-9a-f]{40} -->$') {
        error make {msg: 'The performance report must start with its comment marker'}
    }
    upsert-comment (repository) (required-env PR_NUMBER | into int) $body --marker $marker
}
