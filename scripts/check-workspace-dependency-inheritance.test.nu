#!/usr/bin/env nix
#! nix shell --inputs-from .. nixpkgs#nushell --command nu

use std/assert

def main []: nothing -> nothing {
    let fixture = mktemp -d
    try {
        '[workspace]
members = ["crates/*", "extras/git"]

[workspace.dependencies]
rustix = { version = "1.1.6", features = ["process"] }
stats_alloc = "0.1.10"

[workspace.lints.rust]
unsafe_code = "deny"
'
        | save ($fixture | path join Cargo.toml)
        mkdir ($fixture | path join crates/inline)
        mkdir ($fixture | path join crates/inherited)
        mkdir ($fixture | path join extras/git)
        mkdir ($fixture | path join adapters/skipped)
        '[dev-dependencies]
stats_alloc = "0.1.10"

[target.unix.dependencies]
rustix = { version = "1.1.6", features = ["process"] }
'
        | save ($fixture | path join crates/inline/Cargo.toml)
        '[dependencies]
rustix.workspace = true

[dev-dependencies]
stats_alloc = { workspace = true, features = [] }
self = { path = "." }

[lints]
workspace = true
'
        | save ($fixture | path join crates/inherited/Cargo.toml)
        '[dependencies]
rustix = { git = "https://example.invalid/rustix" }
'
        | save ($fixture | path join extras/git/Cargo.toml)
        '[dependencies]
stats_alloc = "0.1.10"
'
        | save ($fixture | path join adapters/skipped/Cargo.toml)
        let check = $env.FILE_PWD | path join check-workspace-dependency-inheritance.nu
        let failing = nu $check $fixture | complete
        assert equal $failing.exit_code 1 $failing.stderr
        assert equal (
            $failing.stderr
            | str contains 'crates/inline/Cargo.toml: stats_alloc must inherit'
        ) true
        assert equal (
            $failing.stderr
            | str contains 'crates/inline/Cargo.toml: rustix must inherit'
        ) true
        assert equal (
            $failing.stderr
            | str contains 'extras/git/Cargo.toml: rustix must inherit'
        ) true
        assert equal ($failing.stderr | str contains 'crates/inherited') false
        assert equal ($failing.stderr | str contains 'adapters/skipped') false
        assert equal (
            $failing.stderr
            | str contains 'crates/inline/Cargo.toml: must inherit [workspace.lints]'
        ) true
        '[dev-dependencies]
stats_alloc.workspace = true

[target.unix.dependencies]
rustix.workspace = true

[lints]
workspace = true
'
        | save --force ($fixture | path join crates/inline/Cargo.toml)
        '[dependencies]
rustix.workspace = true

[lints]
workspace = true
'
        | save --force ($fixture | path join extras/git/Cargo.toml)
        let passing = nu $check $fixture | complete
        assert equal $passing.exit_code 0 $passing.stderr

        open --raw ($fixture | path join Cargo.toml)
        | str replace 'unsafe_code = "deny"' 'unsafe_code = "allow"'
        | save --force ($fixture | path join Cargo.toml)
        let lenient = nu $check $fixture | complete
        assert equal $lenient.exit_code 1 $lenient.stderr
        assert equal (
            $lenient.stderr
            | str contains 'Cargo.toml: [workspace.lints.rust] unsafe_code must be'
        ) true
    } catch {|err|
        rm -rf $fixture
        error make $err
    }
    rm -rf $fixture
    print 'workspace dependency inheritance tests passed.'
}
