#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell nixpkgs#llvm --command nu

use ./profile-instructions/core.nu [resolve-ref]
use ./binary-size/core.nu

def main []: nothing -> nothing {
    print --stderr 'usage: binary-size.nu <plan|build|compare|check> ...'
    exit 1
}

def github-output [values: record]: nothing -> nothing {
    $values
    | items {|key value| $"($key)=($value | to json --raw)\n" }
    | str join
    | save --append $env.GITHUB_OUTPUT
}

# Resolve the dispatch refs to commits and emit one build per ref and target.
def "main plan" [--refs: string = 'main', --targets: string = '']: nothing -> nothing {
    let ref_rows = core ref-rows $refs | each {|row| $row | insert sha (resolve-ref $row.ref --fetch) }
    let selected = core select-targets $targets
    let builds = core matrix $ref_rows $selected
    github-output {
        refs: $ref_rows
        targets: ($selected | get target)
        builds: $builds
    }
    $builds | select id ref sha target | table --expand | print --stderr
}

# Build the shipped release derivation of one checkout, plus a twin that keeps its symbol table
# when `--symbols` is set, and record the sizes of both. A commit older than `--package` is
# measured through `--fallback`, the attribute it does ship.
def "main build" [
    --source: path
    --ref: string
    --sha: string
    --target: string
    --package: string
    --fallback: string
    --symbols
    --out: path
]: nothing -> nothing {
    let system = ^nix eval --impure --raw --expr builtins.currentSystem
    let flake = $"git+file://($source | path expand)?shallow=1&rev=($sha)"
    let available = (
        ^nix eval --json $"($flake)#packages.($system)" --apply builtins.attrNames
        | from json
    )
    let attribute = core resolve-attribute $package $fallback $available
    if $attribute != $package {
        print --stderr $"($package) is missing in ($sha); using ($attribute)"
    }
    let attrs = if $symbols { [shipped symbols] } else { [shipped] }
    print --stderr $"build: ($flake)#($attribute) \(($attrs | str join ', ')\)"
    let built = (
        ^nix build --no-link --json
        --file ($env.FILE_PWD | path join binary-size/release.nix)
        --argstr flake $flake
        --argstr system $system
        --argstr package $attribute
        ...$attrs
        | from json
        | each { get outputs.out | path join bin/ccusage }
    )
    let shipped = $built | first
    let sections = core parse-sections (^llvm-size -A $shipped)
    let symbols = match $built {
        [_, $unstripped] => (
            core parse-symbols (^llvm-nm --size-sort -S --demangle --defined-only $unstripped)
        )
        _ => null
    }
    mkdir $out
    {
        ref: $ref
        sha: $sha
        target: $target
        package: $package
        attribute: $attribute
        bytes: (
            ls $shipped
            | first
            | get size
            | into int
        )
        sections: (core key-sections $sections)
        section_table: $sections
        symbols: $symbols
    } | to json | save --force ($out | path join build.json)
}

# Compare every build against its target's base and write `report.json` and `report.md`.
def "main compare" [
    --dir: path
    --refs: string
    --targets: string
    --limit: string = ''
    --out: path
]: nothing -> nothing {
    let report = (
        core report
        ($refs | from json)
        (core select-targets ($targets | from json | str join ' '))
        (glob ($dir | path join '**/build.json') | each { open })
        (core parse-limit $limit)
    )
    mkdir $out
    $report | to json | save --force ($out | path join report.json)
    core report-md $report | save --force ($out | path join report.md)
}

# Fail when any binary in the report exceeds the limit.
def "main check" [report: path]: nothing -> nothing {
    let over = core over-limit (open $report)
    if ($over | is-not-empty) {
        error make --unspanned {msg: $"Over the size limit: ($over | str join ', ')"}
    }
}
