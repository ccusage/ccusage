#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell --command nu

export def discover-native-package-versions []: nothing -> list<record> {
    let packages = (
        glob 'packages/ccusage-*/package.json'
        | each {|manifest_path|
            let manifest = open $manifest_path
            {
                name: $manifest.name
                version: $manifest.version
            }
        }
        | sort-by name
    )
    if ($packages | is-empty) {
        error make {msg: 'No native package manifests found under packages/ccusage-*'}
    }
    $packages
}

export def wait-for-npm-package-versions [
    packages: list<record>
    --npm-command: string = 'npm'
    --timeout: duration
    --poll-interval: duration
    --fetch-timeout-ms: int = 10000
]: nothing -> nothing {
    let effective_timeout = $timeout | default 4min
    let effective_poll_interval = $poll_interval | default 10sec
    let deadline = (date now) + $effective_timeout
    let proportional_overhead = $effective_timeout / 10
    let round_overhead = if $proportional_overhead < 10sec { $proportional_overhead } else { 10sec }
    mut pending = $packages

    while (not ($pending | is-empty)) and (
        (date now) + (($pending | length) * $fetch_timeout_ms * 1ms) + $round_overhead < $deadline
    ) {
        let results = (
            $pending
            | each {|package|
                let package_spec = $"($package.name)@($package.version)"
                let result = (
                    run-external $npm_command view $package_spec version
                        '--fetch-retries=0'
                        $"--fetch-timeout=($fetch_timeout_ms)"
                    | complete
                )
                {
                    package: $package
                    available: (($result.exit_code == 0) and (($result.stdout | str trim) == $package.version))
                }
            }
        )
        $pending = ($results | where {|result| not $result.available} | get package)

        if not ($pending | is-empty) {
            let names = $pending | get name | str join ', '
            print --stderr $"Waiting for npm registry to serve exact versions: ($names)"

            let remaining = $deadline - (date now)
            if $remaining > 0sec {
                let delay = if $remaining < $effective_poll_interval { $remaining } else { $effective_poll_interval }
                sleep $delay
            }
        }
    }

    if not ($pending | is-empty) {
        let versions = (
            $pending
            | each {|package| $"($package.name)@($package.version)"}
            | str join ', '
        )
        error make {msg: $"npm registry did not serve exact package versions within ($effective_timeout): ($versions)"}
    }

    print 'All native package versions are available from npm.'
}

def main []: nothing -> nothing {
    let packages = discover-native-package-versions
    wait-for-npm-package-versions $packages
}
