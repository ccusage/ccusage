#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell nixpkgs#bash nixpkgs#jq --command nu

use std/assert

def has-run-step [steps: list<any>]: nothing -> bool {
    $steps | any {|step|
        (
            (($step | get --optional run) != null)
            or (has-run-step ($step | get --optional parallel | default []))
        )
    }
}

def main []: nothing -> nothing {
    let workflows = glob ($env.FILE_PWD | path join ../workflows/*.y*ml)
    $workflows | each {|file|
        let workflow = open $file
        let has_run = $workflow.jobs | values | any {|job|
            has-run-step ($job | get --optional steps | default [])
        }
        if $has_run {
            assert equal ($workflow | get --optional defaults.run.shell) bash $"($file) must default run steps to bash so pipelines fail with their first command"
        }
    } | ignore

    let step = open ($env.FILE_PWD | path join ../workflows/update-pricing.yaml)
    | get jobs.update-pricing.steps
    | where {|step| ($step | get --optional id) == 'update' }
    | first
    let fixture = mktemp -d
    try {
        let script = $fixture | path join update.nu
        [
            $"#!($nu.current-exe)"
            'def main [] {'
            '    if $env.TEST_FAIL == "true" { exit 3 }'
            '    {changed: true, paths: [flake.lock rust/a.json]} | to json --raw | print'
            '}'
        ] | str join "\n" | save $script
        ^chmod +x $script
        let output = $fixture | path join output.txt
        [
            {
                fail: 'true'
                valid: false
                output: []
            }
            {
                fail: 'false'
                valid: true
                output: ['changed=true' 'paths=flake.lock rust/a.json']
            }
        ] | each {|case|
            '' | save --force $output
            let result = with-env {
                SCRIPT: $script
                GITHUB_OUTPUT: $output
                TEST_FAIL: $case.fail
            } {
                bash --noprofile --norc -eo pipefail -c $step.run | complete
            }
            assert equal ($result.exit_code == 0) $case.valid $result.stderr
            assert equal (open --raw $output | lines) $case.output
        } | ignore
    } catch {|err|
        rm -rf $fixture
        error make $err
    }
    rm -rf $fixture
    print 'Workflow shell tests passed.'
}
