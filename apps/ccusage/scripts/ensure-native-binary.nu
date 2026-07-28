#!/usr/bin/env nix
#! nix shell --inputs-from ../../.. nixpkgs#nushell --command nu

use ./native-binary.nu [binary-name, linked-dylibs]

const system_dylib_prefixes = ['/usr/lib/', '/System/Library/']
def main [] {
    let repo_root = (
        $env.CURRENT_FILE | path dirname | path join ../../.. | path expand
    )
    let target_platform = (node_platform)
    let target_arch = (node_arch)
    let binary_name = (binary-name $target_platform)
    let native_package_root = (matching_native_package_root $repo_root $target_platform $target_arch)
    let native_binary = (match $native_package_root {
        null => null
        $package_root => ($package_root | path join bin $binary_name)
    })
    let cargo_binary = $repo_root | path join rust target release $binary_name
    let version = (expected_version $repo_root)
    if (native_package_includes_binary $native_package_root $binary_name) and (has_expected_version $native_binary $version) {
        if not (is_portable_binary $target_platform $native_binary) {
            error make {msg: $"($native_binary) depends on dynamic libraries that do not exist on end-user machines; rebuild it \(Linux packages must be static, macOS packages may only link system dylibs)"}
        }
        exit 0
    }
    ^cargo build --manifest-path ($repo_root | path join rust Cargo.toml) --release --bin ccusage
    if not (has_expected_version $cargo_binary $version) {
        error make {msg: $"($cargo_binary) did not report version ($version) after cargo build"}
    }
}
def node_platform [] {
    match $nu.os-info.name {
        'macos' => 'darwin'
        'linux' => 'linux'
        'windows' => 'win32'
        $other => $other
    }
}
def node_arch [] {
    match $nu.os-info.arch {
        'aarch64' => 'arm64'
        'x86_64' => 'x64'
        $other => $other
    }
}
def matching_native_package_root [repo_root: path, target_platform: string, target_arch: string] {
    glob ($repo_root | path join packages 'ccusage-*' package.json)
    | where {|package_json_path|
        let package_json = (open $package_json_path)
        (list_contains $package_json.os? $target_platform) and (list_contains $package_json.cpu? $target_arch)
    }
    | each {|package_json_path| $package_json_path | path dirname }
    | get --optional 0
}
def list_contains [value, needle: string] { (($value | describe) =~ '^list') and ($value | any {|item| $item == $needle }) }
def expected_version [repo_root: path] {
    let version = open ($repo_root | path join apps ccusage package.json) | get --optional version
    match ($version | describe) {
        'string' => $version
        _ => (error make {msg: 'apps/ccusage/package.json version is not configured'})
    }
}
def native_package_includes_binary [package_root, binary_name: string] {
    match $package_root {
        null => false
        $root => (manifest_lists_binary ($root | path join package.json) $binary_name)
    }
}
def manifest_lists_binary [package_json_path: path, binary_name: string] {
    ($package_json_path | path exists) and (
        list_contains (open $package_json_path | get --optional files) $"bin/($binary_name)"
    )
}
def is_portable_binary [target_platform: string, binary] {
    match [$target_platform, $binary] {
        [_, null] => false
        ['linux', _] => (linux_binary_is_static $binary)
        ['darwin', _] => (darwin_binary_links_only_system_dylibs $binary)
        _ => true
    }
}
def linux_binary_is_static [binary: path] {
    let result = ^ldd $binary | complete
    $"($result.stdout)($result.stderr)" =~ '(?i)not a dynamic executable|statically linked'
}
def darwin_binary_links_only_system_dylibs [binary: path] {
    let linked = (linked-dylibs $binary)
    $linked.ok and (
        $linked.dylibs
        | all {|dylib| $system_dylib_prefixes | any {|prefix| $dylib | str starts-with $prefix } }
    )
}
def has_expected_version [binary, version: string] {
    ($binary != null) and ($binary | path exists) and ((reported_version $binary) == $version)
}
def reported_version [binary: path] {
    let result = run-external $binary '--version' | complete
    match $result.exit_code {
        0 => (
            $result.stdout
            | str trim
            | split row --regex '\s+'
            | last
        )
        _ => null
    }
}
