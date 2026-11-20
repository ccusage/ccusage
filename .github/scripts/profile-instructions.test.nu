#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell --command nu

use std/assert
use ./profile-instructions/core.nu

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
    command: string
    total: int
    self: list<any>
]: nothing -> record {
    {
        ref: $ref
        sha: ('0' | fill --character '0' --width 40)
        command: $command
        fixture: large
        total: $total
        self: $self
        inclusive: $self
    }
}

def main []: nothing -> nothing {
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

    assert equal (core parse-command 'claude daily') {command: 'claude daily' fixture: large}
    assert equal (core parse-command 'daily --breakdown') {command: 'daily --breakdown' fixture: large}
    assert equal (core parse-command 'amp daily') {command: 'amp daily' fixture: golden}
    assert equal (core parse-command 'golden: claude  session') {command: 'claude session' fixture: golden}

    let plan = core plan 'main, feature/x main' "claude daily\namp daily, claude daily"
    assert equal $plan.refs [
        {ref: main, slug: main}
        {ref: feature/x, slug: feature-x}
    ]
    assert equal ($plan.runs.id) [
        main--claude-daily
        main--amp-daily
        feature-x--claude-daily
        feature-x--amp-daily
    ]
    assert equal ((core plan main '').runs | length) ($core.default_commands | length)

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
            command: 'claude daily'
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
    print 'Profile instruction tests passed.'
}
