#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell --command nu

use std/assert
use ./profile-instructions/core.nu

const commit_env = {
    GIT_AUTHOR_NAME: 'Profile Test'
    GIT_AUTHOR_EMAIL: 'profile-test@example.com'
    GIT_COMMITTER_NAME: 'Profile Test'
    GIT_COMMITTER_EMAIL: 'profile-test@example.com'
}

const annotate_self = '--------------------------------------------------------------------------------
Profile data file ''callgrind.out'' (creator: callgrind-3.25.1)
--------------------------------------------------------------------------------
Events recorded:  Ir
Thresholds:       100

--------------------------------------------------------------------------------
Ir
--------------------------------------------------------------------------------
1,000 (100.0%)  PROGRAM TOTALS

--------------------------------------------------------------------------------
Ir           file:function
--------------------------------------------------------------------------------
500 (50.00%)  ???:memchr::arch::x86_64::memchr::memchr_raw::find_avx2 [/tmp/bin/ccusage]
300 (30.00%)  /build/rust/crates/core/src/lib.rs:ccusage_core::parse::line::h0123456789abcdef [/tmp/bin/ccusage]
100 (10.00%)  /other/lib.rs:ccusage_core::parse::line::hfedcba9876543210 [/tmp/bin/ccusage]
 60           ???:<alloc::vec::Vec<T> as core::ops::drop::Drop>::drop [/tmp/bin/ccusage]
 40 ( 4.00%)  ???:0x00000000000ab650 [/usr/lib/x86_64-linux-gnu/libc.so.6]
'

def fake-run [
    ref: string
    label: string
    total: int
    self: list<any>
    --fixture: string = 'large'
]: nothing -> record {
    {
        ref: $ref
        sha: ('0' | fill --character '0' --width 40)
        label: $label
        fixture: $fixture
        total: $total
        self: $self
        inclusive: $self
    }
}

def ambiguous-prefix [repo: string]: nothing -> string {
    mut prefixes = []
    mut prefix = ''
    mut tries = 0
    while $prefix == '' {
        let sha = $"candidate ($tries)" | ^git -C $repo hash-object -w --stdin | str trim
        let candidate = $sha | str substring 0..<4
        if $candidate in $prefixes {
            $prefix = $candidate
        } else {
            $prefixes = $prefixes | append $candidate
        }
        $tries += 1
        if $tries > 4096 {
            error make {msg: 'Could not build two objects sharing four hex digits'}
        }
    }
    $prefix
}

# NOTE(2026-11-20): A developer's git config must not reach these fixtures, or a global `tag.gpgSign` breaks `git tag`. Clearing it also covers the `git` calls inside `resolve-ref`, and identities still come from `commit_env`.
def main []: nothing -> nothing {
    $env.GIT_CONFIG_GLOBAL = '/dev/null'
    $env.GIT_CONFIG_NOSYSTEM = '1'
    let parsed = core parse-annotate $annotate_self
    assert equal $parsed.total 1000
    assert equal ($parsed.functions | select function ir) [
        {
            function: 'memchr::arch::x86_64::memchr::memchr_raw::find_avx2'
            ir: 500
        }
        {function: 'ccusage_core::parse::line', ir: 400}
        {
            function: '<alloc::vec::Vec<T> as core::ops::drop::Drop>::drop'
            ir: 60
        }
        {function: 'libc.so.6+0x00000000000ab650', ir: 40}
    ]
    assert equal (
        $parsed.functions
        | where ir == 400
        | first
        | get file
    ) '/build/rust/crates/core/src/lib.rs'

    assert equal (core parse-command 'claude daily') {
        args: 'claude daily'
        fixture: large
        label: 'claude daily'
    }
    assert equal (core parse-command 'daily --breakdown') {
        args: 'daily --breakdown'
        fixture: large
        label: 'daily --breakdown'
    }
    assert equal (core parse-command 'amp daily') {
        args: 'amp daily'
        fixture: golden
        label: 'amp daily'
    }
    assert equal (core parse-command 'golden: claude  session') {
        args: 'claude session'
        fixture: golden
        label: 'golden:claude session'
    }
    assert equal (core parse-command 'many-files:claude daily') {
        args: 'claude daily'
        fixture: 'many-files'
        label: 'many-files:claude daily'
    }
    assert equal (core parse-command 'proxy:claude daily') {
        args: 'claude daily'
        fixture: proxy
        label: 'proxy:claude daily'
    }
    assert equal (core parse-command 'large:amp daily') {
        args: 'amp daily'
        fixture: large
        label: 'large:amp daily'
    }

    assert equal (core labeled-entry 'golden:codex daily' golden) {
        args: 'codex daily'
        fixture: golden
        label: 'golden:codex daily'
    }
    assert equal (core labeled-entry 'claude daily' 'many-files') {
        args: 'claude daily'
        fixture: 'many-files'
        label: 'many-files:claude daily'
    }
    assert equal (core labeled-entry 'claude daily' large) {
        args: 'claude daily'
        fixture: large
        label: 'claude daily'
    }
    assert error { core labeled-entry 'golden:codex daily' large }

    let relabelled = core plan main 'codex daily, golden:codex daily, many-files:claude daily, proxy:claude daily, daily, golden:daily'
    let rebuilt = $relabelled.runs | each {|run| core labeled-entry $run.args $run.fixture | get label }
    assert equal $rebuilt $relabelled.runs.label
    assert equal ($rebuilt | uniq | length) ($rebuilt | length)

    let plan = core plan 'main, feature/x main' "claude daily\namp daily, claude daily, many-files:claude daily"
    assert equal $plan.refs [
        {ref: main, slug: main}
        {ref: feature/x, slug: feature-x}
    ]
    assert equal ($plan.runs.id) [
        main--claude-daily
        main--amp-daily
        main--many-files-claude-daily
        feature-x--claude-daily
        feature-x--amp-daily
        feature-x--many-files-claude-daily
    ]
    assert equal ((core plan main '').runs | length) ($core.default_commands | length)

    let both_fixtures = core plan main 'codex daily, golden:codex daily'
    assert equal ($both_fixtures.runs | select label fixture id) [
        {label: 'codex daily', fixture: large, id: main--codex-daily}
        {
            label: 'golden:codex daily'
            fixture: golden
            id: main--golden-codex-daily
        }
    ]

    let since_dir = mktemp -d
    try {
        [202606010000 202606040000 202606080000 202606130000 202606210000] | enumerate | each {|row|
            let file = $since_dir | path join projects $"p($row.index)" s.jsonl
            mkdir ($file | path dirname)
            '{}' | save $file
            with-env {TZ: UTC} { ^touch -t ($row.item | into string) $file }
        } | ignore
        let since = core resolve-args $"claude daily --since ($core.median_mtime)" $since_dir
        assert equal $since 'claude daily --since 20260608'
        let cut = $since | split row ' ' | last
        let pruned = glob ($since_dir | path join '**/*.jsonl')
        | where {|file| (ls --long $file | first | get modified | date to-timezone UTC | format date '%Y%m%d') < $cut }
        | length
        assert ($pruned > 0 and $pruned < 5) 'the median-mtime cut-off prunes some but not all files'
        assert equal (core resolve-args 'claude daily' $since_dir) 'claude daily'
        assert error { core resolve-args $"claude daily --since ($core.median_mtime)" ($since_dir | path join none) }
    } catch {|error|
        rm --recursive --force $since_dir
        error make $error
    }
    rm --recursive --force $since_dir

    let shared = {function: shared, ir: 10}
    let base = [
        (fake-run main 'claude daily' 100 [
            {function: big, ir: 60}
            $shared
        ])
        (fake-run main 'amp daily' 10 [
            $shared
            {function: small, ir: 1}
        ])
    ]
    assert equal ((core rank $base).function) [shared big small]

    let head = [
        (
            fake-run head 'claude daily' 80 [
                {function: big, ir: 40}
                $shared
                {function: fresh, ir: 5}
            ]
        )
        (fake-run head 'codex daily' 50 [
            {function: big, ir: 50}
        ])
    ]
    let result = core deltas $base $head
    assert equal $result.commands [
        {
            label: 'claude daily'
            base: 100
            head: 80
            delta: -20
            ratio: 0.8
        }
    ]
    assert equal $result.functions [
        {
            function: big
            base: 60
            head: 40
            delta: -20
        }
        {
            function: fresh
            base: 0
            head: 5
            delta: 5
        }
    ]
    assert str contains (core deltas-md $result main head) '| `big` | 60 | 40 | -20 |'
    assert str contains (core section-md main $base (core rank $base)) '## Hot functions at `main` (000000000000)'
    assert str contains (core section-md missing [] []) '## Hot functions at `missing` (no completed runs)'

    let base_fixtures = [
        (fake-run main 'codex daily' 100 [
            {function: shared, ir: 10}
        ])
        (fake-run main 'golden:codex daily' 200 [
            {function: shared, ir: 20}
        ] --fixture golden)
    ]
    let head_fixtures = [
        (fake-run head 'codex daily' 80 [
            {function: shared, ir: 8}
        ])
        (fake-run head 'golden:codex daily' 160 [
            {function: shared, ir: 16}
        ] --fixture golden)
    ]
    assert equal (
        (core deltas $base_fixtures $head_fixtures).commands
        | select label base head
    ) [
        {label: 'codex daily', base: 100, head: 80}
        {label: 'golden:codex daily', base: 200, head: 160}
    ]

    let repo = mktemp -d
    try {
        ^git init --quiet --initial-branch main $repo
        with-env $commit_env { ^git -C $repo commit --quiet --allow-empty --message first }
        ^git -C $repo branch profile
        ^git -C $repo tag release
        let head = ^git -C $repo rev-parse HEAD | str trim
        let short = $head | str substring 0..<8
        assert equal ($head | str length) 40
        assert equal (do {
            cd $repo
            {
                sha: (core resolve-ref $head)
                short: (core resolve-ref $short)
                branch: (core resolve-ref profile)
                tag: (core resolve-ref release)
            }
        }) {
            sha: $head
            short: $head
            branch: $head
            tag: $head
        }
        let missing = do {
            cd $repo
            try { core resolve-ref 0123456789abcdef } catch {|err| $err.msg }
        }
        assert str contains $missing "Cannot resolve ref '0123456789abcdef' to a commit"

        let prefix = ambiguous-prefix $repo
        let ambiguous = do {
            cd $repo
            try { core resolve-ref $prefix } catch {|err| $err.msg }
        }
        assert str contains $ambiguous $"Ref '($prefix)' matches several objects in this checkout"
    } catch {|err|
        rm -rf $repo
        error make $err
    }
    rm -rf $repo

    let remotes = mktemp -d
    let remote = $remotes | path join remote.git
    let writer = $remotes | path join writer
    let reader = $remotes | path join reader
    try {
        ^git init --quiet --initial-branch main $writer
        with-env $commit_env { ^git -C $writer commit --quiet --allow-empty --message first }
        ^git init --quiet --bare --initial-branch main $remote
        ^git -C $writer remote add origin $remote
        ^git -C $writer push --quiet origin main
        ^git clone --quiet $remote $reader
        with-env $commit_env {
            ^git -C $writer commit --quiet --allow-empty --message second
            ^git -C $writer push --quiet origin main
        }
        ^git -C $writer tag remote-only
        ^git -C $writer push --quiet origin remote-only
        let pushed = ^git -C $writer rev-parse HEAD | str trim
        let short = $pushed | str substring 0..<8
        let before = do {
            cd $reader
            try { core resolve-ref $short } catch {|err| $err.msg }
        }
        assert str contains $before $"Cannot resolve ref '($short)'"
        let tag_before = do {
            cd $reader
            try { core resolve-ref remote-only } catch {|err| $err.msg }
        }
        assert str contains $tag_before "Cannot resolve ref 'remote-only'"
        assert equal (do {
            cd $reader
            core resolve-ref $short --fetch
        }) $pushed
        assert equal (do {
            cd $reader
            core resolve-ref remote-only --fetch
        }) $pushed
        let failed = do {
            cd $reader
            try { core resolve-ref deadbeefdeadbeef --fetch } catch {|err| $err.msg }
        }
        assert str contains $failed "Cannot resolve ref 'deadbeefdeadbeef' to a commit"
        assert str contains $failed 'git fetch reported:'
    } catch {|err|
        rm -rf $remotes
        error make $err
    }
    rm -rf $remotes
    print 'Profile instruction tests passed.'
}
