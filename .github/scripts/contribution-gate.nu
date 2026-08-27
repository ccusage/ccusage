#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell nixpkgs#gh --command nu

use ./contribution-gate/access.nu [issue-access pr-access]
use ./contribution-gate/coauthor.nu [verify-coauthor]
use ./contribution-gate/mutations.nu [issue-verdict pr-verdict]
use ./contribution-gate/requests.nu [issue-request pr-request issue-implementation-request]

def main [operation: string]: nothing -> nothing {
    match $operation {
        'issue-access' => issue-access
        'pr-access' => pr-access
        'issue-request' => issue-request
        'pr-request' => pr-request
        'issue-verdict' => issue-verdict
        'pr-verdict' => pr-verdict
        'issue-implementation-request' => issue-implementation-request
        'verify-coauthor' => verify-coauthor
        _ => (error make {msg: $"Unknown contribution-gate operation: ($operation)"})
    }
}
