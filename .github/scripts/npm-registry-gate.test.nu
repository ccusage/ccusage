#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell --command nu

use ./npm-registry-gate.nu [discover-native-package-versions wait-for-npm-package-versions]

def expect [name: string, actual, expected]: nothing -> nothing {
    if $actual != $expected {
        let expected_json = $expected | to json
        let actual_json = $actual | to json
        error make {msg: $"($name): expected ($expected_json), got ($actual_json)"}
    }
}

def stub-environment [directory: string, mode: string]: nothing -> record {
    {
        NPM_STUB_MODE: $mode
        NPM_STUB_STATE_FILE: ($directory | path join $"($mode).count")
        NPM_STUB_CALLS: ($directory | path join $"($mode).calls")
        NPM_STUB_SPECS: ($directory | path join $"($mode).specs")
    }
}

def run-gate [
    packages: list<record>
    stub_path: string
    timeout: duration
    poll_interval: duration
]: nothing -> nothing {
    (wait-for-npm-package-versions
        $packages
        --npm-command $stub_path
        --timeout $timeout
        --poll-interval $poll_interval
        --fetch-timeout-ms 100
    )
}

def test-stub []: nothing -> nothing {
    let temp = run-external mktemp '-d' | complete
    if $temp.exit_code != 0 {
        error make {msg: $"mktemp failed: ($temp.stderr | str trim)"}
    }
    let directory = $temp.stdout | str trim
    let stub_path = $directory | path join 'npm-stub.sh'
    '#!/bin/sh
set -eu
test "$1" = view || exit 64
printf "%s\n" "$*" >> "$NPM_STUB_CALLS"
printf "%s\n" "$2" >> "$NPM_STUB_SPECS"
count=0
if [ -f "$NPM_STUB_STATE_FILE" ]; then
    count=$(cat "$NPM_STUB_STATE_FILE")
fi
count=$((count + 1))
printf "%s\n" "$count" > "$NPM_STUB_STATE_FILE"
version=${2##*@}
case "$NPM_STUB_MODE" in
    success) printf "%s\n" "$version" ;;
    eventual)
        if [ "$count" -lt 7 ]; then exit 1; fi
        printf "%s\n" "$version"
        ;;
    failure) exit 1 ;;
    *) exit 65 ;;
esac
' | save --force $stub_path
    let executable = run-external chmod '+x' $stub_path | complete
    if $executable.exit_code != 0 {
        error make {msg: $"Could not make npm stub executable: ($executable.stderr | str trim)"}
    }

    let packages = discover-native-package-versions
    expect 'discovers all six native package manifests' ($packages | length) 6

    with-env (stub-environment $directory 'success') {
        run-gate $packages $stub_path 2sec 50ms
    }
    let success_calls = (
        open --raw ($directory | path join 'success.calls')
        | lines
    )
    let success_specs = (
        open --raw ($directory | path join 'success.specs')
        | lines
        | sort
    )
    let expected_specs = (
        $packages
        | each {|package| $"($package.name)@($package.version)"}
        | sort
    )
    (expect
        'queries every native package once when already visible'
        ($success_calls | length)
        6
    )
    expect 'queries all exact manifest versions' $success_specs $expected_specs
    expect 'queries exact manifest versions with bounded npm settings' (
        $success_calls | all {|call|
            (($call | str contains '--fetch-retries=0') and ($call | str contains '--fetch-timeout=100') and ($packages | any {|package|
                $call | str contains $"($package.name)@($package.version)"
            }))
        }
    ) true

    with-env (stub-environment $directory 'eventual') {
        run-gate $packages $stub_path 2sec 50ms
    }
    let eventual_calls = (
        open --raw ($directory | path join 'eventual.calls')
        | lines
    )
    (expect
        'retries exact versions that appear on a later poll'
        ($eventual_calls | length)
        12
    )

    let failure_message = (try {
        with-env (stub-environment $directory 'failure') {
            run-gate $packages $stub_path 2sec 50ms
        }
        'unexpected success'
    } catch {|error|
        $error.msg
    })
    expect 'fails when exact versions never appear before the deadline' (
        $failure_message | str starts-with 'npm registry did not serve exact package versions within'
    ) true
    let failure_calls = (
        open --raw ($directory | path join 'failure.calls')
        | lines
    )
    (expect
        'queries the registry before a permanent failure'
        (($failure_calls | length) > 0)
        true
    )

    let cleanup = run-external rm '-r' $directory | complete
    if $cleanup.exit_code != 0 {
        error make {msg: $"Could not remove test directory: ($cleanup.stderr | str trim)"}
    }
    print 'npm registry gate stub tests passed.'
}

def main []: nothing -> nothing {
    test-stub
}
