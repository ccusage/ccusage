#!/usr/bin/env nix
#! nix shell --inputs-from ../../.. nixpkgs#nushell --command nu

use std/assert
use ./native-package-telemetry.nu verify_telemetry_endpoint

def test_enabled_binary_with_endpoint_passes [binary_path: path]: nothing -> nothing {
    assert equal (verify_telemetry_endpoint $binary_path true) null 'enabled build contains the endpoint'
}

def test_enabled_binary_without_endpoint_fails [binary_path: path]: nothing -> nothing {
    let issue = try {
        verify_telemetry_endpoint $binary_path true
        null
    } catch {|error|
        $error.msg
    }
    assert equal $issue 'Native package binary is missing the telemetry endpoint' 'enabled build must contain the endpoint'
}

def test_disabled_binary_with_endpoint_fails [binary_path: path]: nothing -> nothing {
    let issue = try {
        verify_telemetry_endpoint $binary_path false
        null
    } catch {|error|
        $error.msg
    }
    assert equal $issue 'Native package binary unexpectedly contains the telemetry endpoint' 'disabled build must not contain the endpoint'
}

def test_disabled_binary_without_endpoint_passes [binary_path: path]: nothing -> nothing {
    assert equal (verify_telemetry_endpoint $binary_path false) null 'disabled build without the endpoint passes'
}

def main []: nothing -> nothing {
    let fixture = mktemp -d
    let endpoint_binary = $fixture | path join endpoint.bin
    let ordinary_binary = $fixture | path join ordinary.bin
    let endpoint = 'https://telemetry.ccusage.com/v1/ping' | into binary
    try {
        [
            0x[00 ff 80]
            $endpoint
            0x[00 01]
        ] | bytes collect | save --raw $endpoint_binary
        'local build' | into binary | save --raw $ordinary_binary

        test_enabled_binary_with_endpoint_passes $endpoint_binary
        test_enabled_binary_without_endpoint_fails $ordinary_binary
        test_disabled_binary_with_endpoint_fails $endpoint_binary
        test_disabled_binary_without_endpoint_passes $ordinary_binary
    } catch {|error|
        rm --recursive --force $fixture
        error make $error
    }
    rm --recursive --force $fixture
    print 'Native package telemetry verification tests passed.'
}
