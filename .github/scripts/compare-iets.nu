#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell nixpkgs#hyperfine --command nu

const DRV_PATHS = 'builtins.mapAttrs (_: check: check.drvPath)'

# Check that iets matches Nix on this flake, and time both.
def main [
    iets: path # the iets binary
    --build # also time a no-op build of every check; they must already be built
    --runs: int = 5 # hyperfine runs per command
]: nothing -> nothing {
    let iets = $iets | path expand
    let system = ^nix eval --impure --raw --expr builtins.currentSystem
    let checks = $".#checks.($system)"
    let shell = $".#devShells.($system).default"
    let tmp = mktemp --directory

    # A fresh CI job has no eval cache, so time both uncached.
    $env.IETS_CACHE = '0'

    let parity = [
        (compare-checks $iets $checks)
        (compare-dev-env $iets $shell $tmp)
    ]

    let check_names = ^nix eval --json $checks --apply builtins.attrNames | from json
    let benchmarks = [
        {
            title: $"Evaluate every check \(($check_names | length) derivations)"
            nix: $"nix eval --option eval-cache false --json ($checks) --apply '($DRV_PATHS)'"
            iets: $"($iets) eval-jobs --flake ($checks)"
        }
        {
            title: 'Print the dev shell environment'
            nix: $"nix print-dev-env --option eval-cache false ($shell)"
            iets: $"($iets) print-dev-env ($shell)"
        }
    ]
    | append (
        if $build {
            [
                {
                    title: 'Build every check (all already built)'
                    nix: $"nix build --no-link --option eval-cache false ($check_names | each {|name| $'($checks).($name)' } | str join ' ')"
                    iets: $"($iets) build ($checks)"
                }
            ]
        } else { [] }
    )
    | enumerate
    | each {|it| benchmark $it.item $"($tmp)/bench-($it.index)" $runs }

    let report = [
        $"## Nix vs iets on `($system)`"
        ''
        $"(^nix --version) against (^$iets version)."
        ''
        '### Same output'
        ''
        ($parity | select subject identical detail | to md)
        ''
        '### Timing (uncached)'
        ''
        ...$benchmarks
    ]
    | str join "\n"

    match ($env.GITHUB_STEP_SUMMARY? | default '') {
        '' => { print $report }
        $summary => {
            $report + "\n" | save --append $summary
        }
    }

    let mismatches = $parity | where not identical
    if ($mismatches | is-not-empty) {
        error make {msg: $"iets disagrees with Nix on: ($mismatches.subject | str join ', ')"}
    }
}

def compare-checks [iets: path, checks: string]: nothing -> record {
    let nix = ^nix eval --json $checks --apply $DRV_PATHS | from json
    # A job that fails to evaluate has `error` instead of `drvPath`.
    let jobs = ^$iets eval-jobs --flake $checks | lines | each { from json }
    let iets_drvs = $jobs
    | each {|job| {($job.attr): ($job.drvPath? | default $"error: ($job.error?)")} }
    | into record
    let differing = $nix
    | columns
    | append ($iets_drvs | columns)
    | uniq
    | where {|name| ($nix | get --optional $name) != ($iets_drvs | get --optional $name) }
    {
        subject: $"`checks` drvPaths \(($nix | columns | length))"
        identical: ($differing | is-empty)
        detail: (
            if ($differing | is-empty) { '' } else {
                $differing | str join ', '
            }
        )
    }
}

def compare-dev-env [iets: path, shell: string, tmp: path]: nothing -> record {
    ^nix print-dev-env $shell | save $"($tmp)/nix-dev-env.sh"
    ^$iets print-dev-env $shell | save $"($tmp)/iets-dev-env.sh"
    let identical = (^cmp $"($tmp)/nix-dev-env.sh" $"($tmp)/iets-dev-env.sh" | complete).exit_code == 0
    {
        subject: '`print-dev-env` output'
        identical: $identical
        detail: (
            if $identical { $"(ls $'($tmp)/nix-dev-env.sh' | get 0.size) byte for byte" } else { 'differs' }
        )
    }
}

def benchmark [case: record<title: string, nix: string, iets: string>, out: path, runs: int]: nothing -> string {
    (
        ^hyperfine --warmup 1 --runs $runs --style basic
        --export-markdown $"($out).md" --export-json $"($out).json"
        --command-name nix $case.nix
        --command-name iets $case.iets
    )
    let means = open $"($out).json" | get results | select command mean | transpose --as-record --header-row
    [
        $"#### ($case.title)"
        ''
        (open --raw $"($out).md")
        $"iets is **($means.nix / $means.iets | math round --precision 1)x** faster than nix"
        ''
    ]
    | str join "\n"
}
