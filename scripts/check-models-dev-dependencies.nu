#!/usr/bin/env nix
#! nix shell --inputs-from .. nixpkgs#nushell --command nu

def main [upstream_lock: path, manifest: path]: nothing -> nothing {
    let packages = open --raw $upstream_lock | from json | get packages
    let dependencies = open --raw $manifest | from json | get dependencies
    $dependencies | transpose name version | each {|dependency|
        let locked = $packages | get $dependency.name | first
        let expected = $"($dependency.name)@($dependency.version)"
        if $locked != $expected {
            error make {msg: $"($dependency.name): manifest requires ($expected), upstream locks ($locked)"}
        }
    } | ignore
}
