#!/usr/bin/env nix
#! nix shell --inputs-from .. nixpkgs#nushell --command nu

def main [rust_dir: path]: nothing -> nothing {
    cd $rust_dir

    let exceptions = {
        "adapters/common/clippy.toml": ["std::fs::read_dir"]
        "crates/ccusage-cli/clippy.toml": ["jiff::tz::TimeZone::system"]
        "crates/ccusage-core/clippy.toml": [
            "std::thread::spawn"
            "std::thread::Scope::spawn"
            "std::thread::Builder::spawn"
        ]
        "crates/ccusage-environment/clippy.toml": ["std::env::var" "std::env::var_os"]
        "crates/ccusage-terminal/clippy.toml": ["std::io::stdout" "std::io::stderr"]
        "crates/ccusage-test-support/clippy.toml": ["std::fs::read_dir"]
    }
    let expected_paths = [clippy.toml] | append ($exceptions | columns) | sort
    let actual_paths = glob "**/{clippy.toml,.clippy.toml}" --exclude ["target/**" "**/target/**"]
    | each {|file| $file | path relative-to (pwd) }
    | sort
    if $actual_paths != $expected_paths {
        let missing = $expected_paths | where {|path| $path not-in $actual_paths }
        let unexpected = $actual_paths | where {|path| $path not-in $expected_paths }
        error make {
            msg: $"Clippy config files differ from the declared set. Missing: ($missing | to json --raw). Unexpected: ($unexpected | to json --raw)."
        }
    }

    let root = open --raw clippy.toml | from toml
    let banned = $root.disallowed-methods | get path
    $exceptions | items {|path, methods|
        let undeclared = $methods | where {|method| $method not-in $banned }
        if ($undeclared | is-not-empty) {
            error make {msg: $"($path) declares exceptions missing from the root config: ($undeclared | str join ', ')"}
        }
        let expected = $root
            | update disallowed-methods {|config|
                $config.disallowed-methods | where {|method| $method.path not-in $methods }
            }
        let actual = open --raw $path | from toml
        if $actual != $expected {
            error make {msg: $"($path) must match rust/clippy.toml after removing ($methods | str join ', ')."}
        }
    } | ignore
}
