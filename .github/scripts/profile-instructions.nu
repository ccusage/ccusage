#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell nixpkgs#valgrind nixpkgs#patchelf --command nu

use ./profile-instructions/core.nu

const report_vars = {
    COLUMNS: '200'
    NO_COLOR: '1'
    TZ: 'UTC'
    LOG_LEVEL: '0'
}

def main []: nothing -> nothing {
    print --stderr 'usage: profile-instructions.nu <plan|build|golden|generate|run|merge> ...'
    exit 1
}

def github-output [values: record]: nothing -> nothing {
    $values
    | items {|key value| $"($key)=($value | to json --raw)\n" }
    | str join
    | save --append $env.GITHUB_OUTPUT
}

def --wrapped run-checked [label: string, command: string, ...args: string]: nothing -> nothing {
    print --stderr $"($label): ($command) ($args | str join ' ')"
    run-external $command ...$args
    if $env.LAST_EXIT_CODE != 0 {
        error make {msg: $"($label) exited with ($env.LAST_EXIT_CODE)"}
    }
}

# Emit the build and profile matrices for the dispatch inputs.
def "main plan" [--refs: string = 'main', --commands: string = '']: nothing -> nothing {
    let plan = core plan $refs $commands
    github-output {refs: $plan.refs, runs: $plan.runs}
    $plan.runs | select id label fixture | table --expand | print --stderr
}

# Build one ref as the shipped release binary plus line tables and symbols, runnable on the host.
def "main build" [ref: string, --out: path]: nothing -> nothing {
    let sha = core resolve-ref $ref --fetch
    let repo = $env.PWD
    let source = mktemp -d | path join src
    run-checked 'worktree' git worktree add --detach $source $sha
    with-env {CARGO_PROFILE_RELEASE_DEBUG: 'line-tables-only' CARGO_PROFILE_RELEASE_STRIP: 'none'} {
        cd $source
        (
            run-checked 'build' nix develop $"git+file://($repo)?rev=($sha)" --command
            cargo build --manifest-path rust/Cargo.toml --release --locked --bin ccusage
        )
    }
    mkdir $out
    let bin = $out | path join ccusage
    cp ($source | path join rust/target/release/ccusage) $bin
    let interpreter = ^patchelf --print-interpreter /bin/sh | str trim
    (run-checked
        'relink'
        patchelf
        --set-interpreter
        $interpreter
        --remove-rpath
        $bin
    )
    run-checked 'smoke' $bin --version
    {ref: $ref, sha: $sha} | to json | save --force ($out | path join build.json)
}

# Export the golden seeds of every agent the matrix tests cover.
def "main golden" [--out: path]: nothing -> nothing {
    mkdir $out
    with-env {CCUSAGE_GOLDEN_FIXTURE_DIR: $out} {
        (
            run-checked 'golden' nix develop --command
            cargo test --manifest-path rust/Cargo.toml --locked --package ccusage --test golden_matrix
            -- --ignored --exact export_golden_fixture
        )
    }
}

# Generate the fixture a non-golden run reads, laid out as `large-env` and `claude-env` expect.
def "main generate" [--fixture: string, --out: path, --mib: int]: nothing -> nothing {
    let generator = $env.FILE_PWD | path join ../../apps/ccusage/scripts/generate-large-fixture.ts
    let claude = [
        --output-dir
        ($out | path join claude)
    ]
    let args = match $fixture {
        'large' => [
            ...$claude
            --codex-output-dir
            ($out | path join codex)
            --grok-output-dir
            ($out | path join grok)
            --size-mib
            ($mib | into string)
            --codex-size-mib
            ($mib | into string)
        ]
        'many-files' => [...$claude --claude-shape many-small-files]
        'proxy' => [
            ...$claude
            --claude-shape
            requestless-proxy
            --size-mib
            ($mib | into string)
        ]
        _ => { error make {msg: $"No generated fixture '($fixture)'"} }
    }
    run-checked 'generate' $generator ...$args
}

def golden-env [dir: path]: nothing -> record<vars: record, cwd: string> {
    let manifest = open ($dir | path join golden-env.json)
    let missing = $manifest.seeded | where {|name|
        $manifest.env | get $name | split row ',' | str trim | all {|store| not ($store | path exists) }
    }
    if ($missing | is-not-empty) {
        error make {
            msg: $"Golden fixture at ($dir) has no store for ($missing | str join ', ')"
        }
    }
    {
        vars: ($manifest.env | reject --optional LOG_LEVEL)
        cwd: $manifest.cwd
    }
}

def large-env [dir: path, home: path]: nothing -> record<vars: record, cwd: string> {
    {
        vars: {
            CLAUDE_CONFIG_DIR: ($dir | path join claude)
            CODEX_HOME: ($dir | path join codex)
            GROK_HOME: ($dir | path join grok)
            HOME: $home
        }
        cwd: $home
    }
}

def claude-env [dir: path, home: path]: nothing -> record<vars: record, cwd: string> {
    {
        vars: {
            CLAUDE_CONFIG_DIR: ($dir | path join claude)
            HOME: $home
        }
        cwd: $home
    }
}

def annotate [file: path, inclusive: bool]: nothing -> string {
    let result = ^callgrind_annotate '--auto=no' '--threshold=100' $"--inclusive=(if $inclusive { 'yes' } else { 'no' })" $file | complete
    if $result.exit_code != 0 {
        error make {msg: $"callgrind_annotate failed: ($result.stderr)"}
    }
    $result.stdout
}

# Run one report under callgrind and write the raw profile, annotations, and ranked summaries.
def "main run" [
    --bin: path
    --command: string
    --fixture: string
    --fixture-dir: path
    --out: path
]: nothing -> nothing {
    mkdir $out
    let build = open ($bin | path dirname | path join build.json)
    let home = mktemp -d
    let context = match $fixture {
        'large' => (large-env $fixture_dir $home)
        'golden' => (golden-env $fixture_dir)
        'many-files' | 'proxy' => (claude-env $fixture_dir $home)
        _ => { error make {msg: $"Unknown fixture '($fixture)'"} }
    }
    let entry = core labeled-entry $command $fixture
    let report_args = core resolve-args $entry.args ($fixture_dir | path join claude)
    let vars = $context.vars | merge $report_vars | merge {PATH: ($env.PATH | str join (char esep))}
    let profile = $out | path join callgrind.out
    let argv = [
        '-i'
        ...($vars | items {|key value| $"($key)=($value)" })
        (which valgrind | first | get path)
        '--tool=callgrind'
        $"--callgrind-out-file=($profile)"
        $"--log-file=($out | path join valgrind.log)"
        '--dump-instr=no'
        '--'
        $bin
        ...($report_args | split row ' ')
        '--offline'
        '--json'
    ]
    print --stderr $"profiling: ccusage ($report_args) --offline --json \(($fixture) fixture\)"
    let result = do {
        cd $context.cwd
        run-external env ...$argv | complete
    }
    if $result.exit_code != 0 {
        let log = $out | path join valgrind.log
        if ($log | path exists) { print --stderr (open --raw $log) }
        error make {
            msg: $"ccusage ($entry.label) exited with ($result.exit_code): ($result.stderr)"
        }
    }
    $result.stdout | save --force ($out | path join report.json)
    let exclusive = annotate $profile false
    let inclusive = annotate $profile true
    $exclusive | save --force ($out | path join annotate-self.txt)
    $inclusive | save --force ($out | path join annotate-inclusive.txt)
    let self_rows = core parse-annotate $exclusive
    let summary = {
        ref: $build.ref
        sha: $build.sha
        args: $entry.args
        label: $entry.label
        fixture: $entry.fixture
        report_bytes: ($result.stdout | str length --utf-8-bytes)
        total: $self_rows.total
        self: $self_rows.functions
        inclusive: ((core parse-annotate $inclusive).functions | first 200)
    }
    $summary | to json | save --force ($out | path join summary.json)
    core run-md $summary | save --force ($out | path join summary.md)
}

# Merge every run summary into one ranking per ref, plus per-function deltas for two refs.
def "main merge" [--dir: path, --refs: string, --out: path]: nothing -> nothing {
    mkdir $out
    let refs = $refs | from json
    let summaries = glob ($dir | path join '**/summary.json') | each { open }
    let sections = $refs | each {|ref|
        let runs = $summaries | where ref == $ref.ref | sort-by label
        let ranked = core rank $runs
        {
            ref: $ref.ref
            runs: $runs
            ranked: $ranked
            markdown: (core section-md $ref.ref $runs $ranked)
        }
    }
    let comparison = match $sections {
        [$base, $head] => (
            core deltas $base.runs $head.runs
            | insert base $base.ref
            | insert head $head.ref
        )
        _ => null
    }
    let markdown = [
        '# Instruction counts'
        ''
        ...($sections.markdown)
        (
            if $comparison == null { '' } else { core deltas-md $comparison $comparison.base $comparison.head }
        )
    ] | str join "\n"
    $markdown | save --force ($out | path join ranking.md)
    {
        refs: (
            $sections
            | each {|section|
                $section
                | reject markdown runs
                | insert commands ($section.runs | select label args fixture total)
            }
        )
        comparison: $comparison
    } | to json | save --force ($out | path join ranking.json)
}
