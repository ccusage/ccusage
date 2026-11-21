#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell --command nu

use std/assert
use ./binary-size/core.nu

const elf_size = '/tmp/ccusage  :
section              size      addr
.rodata            530000   2097152
.eh_frame           40000   2627152
.text             3100000   2670592
.data                 900   5771520
Total             3670900
'

const macho_size = '/tmp/ccusage  :
section                 size         addr
__text               3155592   4294969408
__const               533160   4298129408
__eh_frame             37896   4298784144
__const               124456   4298835312
Total                3851104
'

const nm_output = '0000000000201000 0000000000000010 T ccusage::main::h0123456789abcdef
0000000000202000 0000000000000020 t ccusage::main::hfedcba9876543210
0000000000203000 0000000000000100 t serde_json::de::parse.llvm.123456
0000000000204000 0000000000000000 t empty
0000000000205000 0000000000000040 R anon.1|2
'

def build [
    ref: string
    target: string
    bytes: int
    text: int
    symbols
]: nothing -> record {
    {
        ref: $ref
        sha: $"($ref)0000000000000000"
        target: $target
        package: $"ccusage-release-($target)"
        bytes: $bytes
        sections: {text: $text, rodata: 100, eh_frame: 10}
        section_table: []
        symbols: $symbols
    }
}

def main []: nothing -> nothing {
    assert equal (core select-targets '' | get target) [linux-x64 linux-arm64 darwin-arm64]
    assert equal (core select-targets 'darwin-arm64, linux-x64' | get target) [linux-x64 darwin-arm64]
    assert error { core select-targets 'windows-x64' }

    assert equal (core parse-limit '') null
    assert equal (core parse-limit '4,200,000') 4200000
    assert equal (core parse-limit '4_200_000') 4200000
    assert error { core parse-limit '4MiB' }

    assert equal (core ref-rows 'main, feat/x main') [
        {ref: main, slug: main}
        {ref: feat/x, slug: feat-x}
    ]
    assert error { core ref-rows ' , ' }
    let collision = try {
        core ref-rows 'main feat/x feat-x' | ignore
        ''
    } catch {|err|
        $err.msg
    }
    assert equal $collision 'Refs collide after slugging: feat/x and feat-x; pass one of each'

    let refs = [
        {ref: main, sha: main0000000000000000, slug: main}
        {ref: feat/x, sha: feat/x0000000000000000, slug: feat-x}
    ]
    let matrix = core matrix $refs (core select-targets 'linux-x64 darwin-arm64')
    assert equal ($matrix | get id) [main-linux-x64 main-darwin-arm64 feat-x-linux-x64 feat-x-darwin-arm64]
    assert equal ($matrix | get 3 | select os cache_scope symbols sha) {
        os: blacksmith-12vcpu-macos-26
        cache_scope: build-mac-arm64
        symbols: false
        sha: feat/x0000000000000000
    }

    assert equal (core key-sections (core parse-sections $elf_size)) {text: 3100000 rodata: 530000 eh_frame: 40000}
    let macho = core parse-sections $macho_size
    assert equal ($macho | length) 4
    assert equal (core key-sections $macho) {text: 3155592 rodata: 533160 eh_frame: 37896}
    assert equal (core key-sections []) {text: null rodata: null eh_frame: null}

    assert equal (core parse-symbols $nm_output) [
        {name: 'serde_json::de::parse', size: 256}
        {name: 'anon.1|2', size: 64}
        {name: 'ccusage::main', size: 48}
    ]

    let deltas = core symbol-deltas [
        {name: same, size: 5}
        {name: grow, size: 10}
        {name: shrink, size: 50}
        {name: gone, size: 7}
    ] [
        {name: same, size: 5}
        {name: grow, size: 30}
        {name: shrink, size: 20}
        {name: new, size: 3}
    ] 20
    assert equal $deltas.growths [
        {
            name: grow
            base: 10
            head: 30
            delta: 20
        }
        {
            name: new
            base: 0
            head: 3
            delta: 3
        }
    ]
    assert equal $deltas.shrinks [
        {
            name: shrink
            base: 50
            head: 20
            delta: -30
        }
        {
            name: gone
            base: 7
            head: 0
            delta: -7
        }
    ]
    assert equal (
        (core symbol-deltas [] [
            {name: a, size: 1}
            {name: b, size: 2}
        ] 1).growths
        | get name
    ) [b]

    let builds = [
        (build main linux-x64 1000 800 [
            {name: 'f|g', size: 10}
        ])
        (build feat/x linux-x64 1100 790 [
            {name: 'f|g', size: 30}
        ])
        (build main darwin-arm64 900 700 null)
    ]
    let report = core report $refs (core select-targets 'linux-x64 darwin-arm64') $builds 1050
    let linux = $report.targets | get 0
    assert equal ($linux.rows | select ref delta within_limit) [
        {ref: main, delta: 0, within_limit: true}
        {ref: feat/x, delta: 100, within_limit: false}
    ]
    assert equal ($linux.rows.1.section_deltas) {text: -10 rodata: 0 eh_frame: 0}
    assert equal ($linux.symbols | select base head) [
        {base: main, head: feat/x}
    ]
    let darwin = $report.targets | get 1
    assert equal ($darwin.rows | get built) [true false]
    assert equal $darwin.symbols []
    assert equal (core over-limit $report) ['linux-x64 at feat/x']
    assert equal (
        core over-limit (core report $refs (core select-targets 'linux-x64') $builds null)
    ) []

    let markdown = core report-md $report
    assert str contains $markdown '| `feat/x` | `feat/x0000` | 1,100 | +100 | +10.00% | 790 (-10) | 100 | 10 | ❌ |'
    assert str contains $markdown '| `main` | `main000000` | 1,000 | base |  | 800 | 100 | 10 | ✅ |'
    assert str contains $markdown '| `feat/x` | `feat/x0000` | build failed | | | | | | |'
    assert str contains $markdown '| +20 | 10 | 30 | `f\|g` |'
    assert str contains $markdown '**Top 1 growths**'
    assert str contains $markdown '**No shrinks**'
    assert str contains $markdown 'Limit: 1,050 bytes per binary.'
    print 'binary-size tests passed'
}
