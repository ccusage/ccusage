#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell --command nu
def main [out_dir: string] {
    if ($out_dir | is-empty) {
        error make {msg: 'usage: generate-e2e-fixture.nu <out-dir>'}
    }
    let fixture_dir = [$out_dir, projects, preview-e2e-project, preview-e2e-session] | path join
    mkdir $fixture_dir
    let lines = (0..419 | each {|index|
        let day = ($index mod 28) + 1 | into string | fill --alignment right --character '0' --width 2
        [
            $"(claude_line $index $day 1 '10:00:00.000Z' 'claude-sonnet-4-20250514' 100 50 25 10 '0.12')"
            $"(claude_line $index $day 2 '12:30:00.000Z' 'claude-opus-4-20250514' 200 75 50 20 '0.34')"
        ]
    } | flatten)
    (($lines | str join (char nl)) + (char nl)) | save --force ([$fixture_dir, chat.jsonl] | path join)
}
def claude_line [
    index: int
    day: string
    line_number: int
    time: string
    model: string
    input_tokens: int
    output_tokens: int
    cache_creation_input_tokens: int
    cache_read_input_tokens: int
    cost_usd: string
] { $"{"timestamp":"2026-01-($day)T($time)","sessionId":"preview-e2e-session","message":{"role":"assistant","id":"preview-e2e-msg-($index)-($line_number)","model":"($model)","usage":{"input_tokens":($input_tokens),"output_tokens":($output_tokens),"cache_creation_input_tokens":($cache_creation_input_tokens),"cache_read_input_tokens":($cache_read_input_tokens)}},"requestId":"preview-e2e-req-($index)-($line_number)","costUSD":($cost_usd),"version":"1.0.0"}" }
