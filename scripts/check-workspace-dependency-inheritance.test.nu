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

        mkdir ($fixture | path join crates/ccusage-zstd/src)
        mkdir ($fixture | path join crates/inherited/src/nested)
        '[lints]
workspace = true
'
        | save ($fixture | path join crates/ccusage-zstd/Cargo.toml)
        '#![expect(
    unsafe_code,
    reason = "the one allowed site"
)]
'
        | save ($fixture | path join crates/ccusage-zstd/src/direct.rs)
        '#![forbid(unsafe_code)]
#[deny(unsafe_code)]
fn strict() {
    let text = "allow(unsafe_code)";
}
// allow(unsafe_code)
macro_rules! quoted {
    () => {
        allow(unsafe_code)
    };
}
'
        | save ($fixture | path join crates/inherited/src/strict.rs)
        let allowed = nu $check $fixture | complete
        assert equal $allowed.exit_code 0 $allowed.stderr
        '#![allow(unsafe_code)]
'
        | save ($fixture | path join crates/ccusage-zstd/src/lib.rs)
        '#[cfg_attr(test, expect(dead_code, unsafe_code, reason = "x"))]
fn f() {}
'
        | save ($fixture | path join crates/inherited/src/nested/inner.rs)
        '#![expect(
    unsafe_code,
    reason = "wrapped like direct.rs"
)]
'
        | save ($fixture | path join crates/inherited/src/wrapped.rs)
        '#[cfg_attr(
    test,
    allow(
        dead_code,
        unsafe_code
    )
)]
fn g() {}
'
        | save ($fixture | path join crates/inherited/src/split.rs)
        '#![warn(unsafe_code)]
'
        | save ($fixture | path join crates/inherited/src/warned.rs)
        let attributes = nu $check $fixture | complete
        assert equal $attributes.exit_code 1 $attributes.stderr
        for site in [
            'crates/ccusage-zstd/src/lib.rs:1:'
            'crates/inherited/src/nested/inner.rs:1:'
            'crates/inherited/src/wrapped.rs:2:'
            'crates/inherited/src/split.rs:5:'
            'crates/inherited/src/warned.rs:1:'
        ] {
            assert equal ($attributes.stderr | str contains $site) true $site
        }
        assert equal ($attributes.stderr | str contains 'direct.rs:') false
        assert equal ($attributes.stderr | str contains 'strict.rs:') false
        for file in [
            crates/ccusage-zstd/src/lib.rs
            crates/inherited/src/nested/inner.rs
            crates/inherited/src/wrapped.rs
            crates/inherited/src/split.rs
            crates/inherited/src/warned.rs
        ] {
            rm ($fixture | path join $file)
        }

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
