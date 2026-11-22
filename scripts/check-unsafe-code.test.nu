#!/usr/bin/env nix
#! nix shell --inputs-from .. nixpkgs#nushell nixpkgs#cargo nixpkgs#rustc --command nu

use std/assert

const bypasses = {
    wrapped: '#![expect(
    unsafe_code,
    reason = "wrapped like direct.rs"
)]
'
    commented: '#[allow /* hidden */ (unsafe_code)]
pub fn f() {}
'
    ignored: '// ast-grep-ignore
#[allow(unsafe_code)]
pub fn f() {}
'
    nested: '#[cfg_attr(
    all(),
    allow(
        dead_code,
        unsafe_code
    )
)]
pub fn f() {}
'
    warned: '#![warn(unsafe_code)]
'
    block: 'pub fn f() -> u8 {
    let value = 1;
    unsafe { *std::ptr::addr_of!(value) }
}
'
    gated: '#[cfg(feature = "gated")]
#[allow(unsafe_code)]
pub fn f() {}
'
}

const clean = r##'#![forbid(unsafe_code)]
#[deny(unsafe_code)]
pub fn f() -> &'static str {
    "allow(unsafe_code)"
}
// allow(unsafe_code)
'##

def check [fixture: path]: nothing -> record {
    let script = $env.FILE_PWD | path join check-unsafe-code.nu
    (
        nu --no-config-file $script ($fixture | path join Cargo.toml)
        --target-dir ($fixture | path join target)
    ) | complete
}

def main []: nothing -> nothing {
    let fixture = mktemp -d
    try {
        '[workspace]
members = ["crates/*"]
resolver = "2"
'
        | save ($fixture | path join Cargo.toml)
        for crate in [ccusage-zstd other] {
            mkdir ($fixture | path join crates $crate src)
            $"[package]\nname = \"($crate)\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[features]\ngated = []\n"
            | save ($fixture | path join crates $crate Cargo.toml)
        }
        '#![expect(unsafe_code, reason = "the exempt crate")]
pub fn read(value: &u8) -> u8 {
    unsafe { *std::ptr::from_ref(value) }
}
'
        | save ($fixture | path join crates/ccusage-zstd/src/lib.rs)
        $clean | save ($fixture | path join crates/other/src/lib.rs)
        ^cargo generate-lockfile --offline --quiet --manifest-path ($fixture | path join Cargo.toml)
        let passing = check $fixture
        assert equal $passing.exit_code 0 $passing.stderr
        $bypasses | transpose name source | each {|bypass|
            $bypass.source | save --force ($fixture | path join crates/other/src/lib.rs)
            let failing = check $fixture
            assert equal $failing.exit_code 101 $"($bypass.name): ($failing.stderr)"
            assert equal ($failing.stderr | str contains 'could not compile `other`') true $bypass.name
            assert equal ($failing.stderr | str contains 'could not compile `ccusage-zstd`') false $bypass.name
        } | ignore
    } catch {|err|
        rm -rf $fixture
        error make $err
    }
    rm -rf $fixture
    print 'unsafe code check tests passed.'
}
