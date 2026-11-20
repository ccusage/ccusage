const large_agents = [claude codex grok]
const reports = [daily weekly monthly session blocks]

export const default_commands = [
    'claude daily'
    'claude session'
    'claude blocks'
    'claude monthly'
    'codex daily'
    'codex session'
    'grok daily'
    'daily'
    'opencode daily'
    'amp daily'
    'droid daily'
    'codebuff daily'
    'hermes daily'
    'pi daily'
    'goose daily'
    'kilo daily'
    'copilot daily'
    'gemini daily'
    'antigravity daily'
    'kimi daily'
    'qwen daily'
    'openclaw daily'
    'zcode daily'
    'cline daily'
    'dsh daily'
]

export def slug [value: string]: nothing -> string {
    $value | str replace --all --regex '[^A-Za-z0-9._-]+' '-' | str trim --char '-'
}

export def split-list [value: string]: nothing -> list<string> {
    $value | split row --regex '[\s,]+' | where $it != '' | uniq
}

# An entry is `[large:|golden:]<command>`; without a prefix the generated large fixture covers
# Claude, Codex, Grok, and the unified reports, and every other agent reads the golden seeds.
export def parse-command [entry: string]: nothing -> record<command: string, fixture: string> {
    match (
        $entry
        | str trim
        | parse --regex '^(?:(?<fixture>large|golden):)?\s*(?<command>.+)$'
    ) {
        [$row] => {
            let command = $row.command | split row --regex '\s+' | str join ' '
            let first = $command | split row ' ' | first
            let fixture = match $row.fixture {
                null if ($first in $large_agents or $first in $reports) => 'large'
                null => 'golden'
                $explicit => $explicit
            }
            {command: $command, fixture: $fixture}
        }
        _ => { error make {msg: $"Empty profile command entry: '($entry)'"} }
    }
}

export def plan [refs: string, commands: string]: nothing -> record<refs: list<any>, runs: list<any>> {
    let ref_rows = split-list $refs | each {|ref| {ref: $ref slug: (slug $ref)} }
    if ($ref_rows | is-empty) {
        error make {msg: 'At least one ref is required'}
    }
    if ($ref_rows.slug | uniq | length) != ($ref_rows | length) {
        error make {msg: $"Refs collide after slugging: ($ref_rows.ref | str join ', ')"}
    }
    let entries = $commands | lines | each { split row ',' } | flatten | str trim | where $it != ''
    let command_rows = if ($entries | is-empty) { $default_commands } else { $entries }
    | each {|entry| parse-command $entry }
    | uniq-by command
    let runs = $ref_rows | each {|ref|
        $command_rows | each {|command|
            $command | merge {
                ref: $ref.ref
                ref_slug: $ref.slug
                id: $"($ref.slug)--(slug $command.command)"
            }
        }
    } | flatten
    {refs: $ref_rows, runs: $runs}
}

# `callgrind_annotate` prints `<file>:<function> [<object>]`; legacy Rust symbols keep a hash
# suffix that changes between builds, so drop it to line functions up across refs.
export def split-symbol [raw: string]: nothing -> record<file: string, function: string> {
    let symbol = $raw | str replace --regex '\s+\[[^\]]*\]$' ''
    let parsed = match ($symbol | parse --regex '^(?<file>[^:]*):(?<function>.+)$') {
        [$row] => $row
        _ => {file: '???', function: $symbol}
    }
    {
        file: $parsed.file
        function: ($parsed.function | str replace --regex '::h[0-9a-f]{16}$' '')
    }
}

export def parse-annotate [text: string]: nothing -> record<total: int, functions: list<any>> {
    let rows = $text
    | lines
    | parse --regex '^\s*(?<ir>[0-9][0-9,]*)\s+(?:\([^)]*\)\s+)?(?<name>\S.*?)\s*$'
    | update ir { str replace --all ',' '' | into int }
    let total = match ($rows | where name == 'PROGRAM TOTALS') {
        [$row] => $row.ir
        _ => { error make {msg: 'callgrind_annotate output has no PROGRAM TOTALS line'} }
    }
    let functions = $rows
    | where name != 'PROGRAM TOTALS'
    | each {|row| (split-symbol $row.name) | insert ir $row.ir }
    | group-by function --to-table
    | each {|group|
        {
            function: $group.function
            file: ($group.items | sort-by ir --reverse | first | get file)
            ir: ($group.items.ir | math sum)
        }
    }
    | sort-by ir --reverse
    {total: $total, functions: $functions}
}

def share [ir: int, total: int]: nothing -> float {
    if $total == 0 { 0.0 } else { $ir / $total }
}

# Functions in more commands' top lists rank first, then by self Ir share averaged over every
# command, so a tiny golden run counts as much as a multi-gigabyte one.
export def rank [runs: list<any>, --top: int = 50]: nothing -> list<any> {
    let count = $runs | length
    $runs
    | each {|run|
        $run.self | first $top | each {|row|
            {function: $row.function ir: $row.ir share: (share $row.ir $run.total) command: $run.command}
        }
    }
    | flatten
    | group-by function --to-table
    | each {|group|
        {
            function: $group.function
            commands: ($group.items | length)
            mean_share: (($group.items.share | math sum) / $count)
            max_share: ($group.items.share | math max)
            self_ir: ($group.items.ir | math sum)
        }
    }
    | sort-by commands mean_share --reverse
}

def sum-by-function [runs: list<any>]: nothing -> list<any> {
    $runs
    | each { get self }
    | flatten
    | group-by function --to-table
    | each {|group| {function: $group.function ir: ($group.items.ir | math sum)} }
}

export def deltas [base: list<any>, head: list<any>]: nothing -> record<commands: list<any>, functions: list<any>> {
    let shared = $base.command | where $it in $head.command
    let base_runs = $base | where command in $shared
    let head_runs = $head | where command in $shared
    let commands = $shared | each {|command|
        let before = $base_runs | where command == $command | first | get total
        let after = $head_runs | where command == $command | first | get total
        {command: $command base: $before head: $after delta: ($after - $before) ratio: (share $after $before)}
    }
    let functions = sum-by-function $base_runs
    | rename function base
    | join --outer (sum-by-function $head_runs | rename function head) function
    | default 0 base
    | default 0 head
    | insert delta {|row| $row.head - $row.base }
    | where delta != 0
    | sort-by {|row| $row.delta | math abs } --reverse
    {commands: $commands, functions: $functions}
}

def cell [value: string]: nothing -> string {
    $"`($value | str replace --all '`' "'" | str replace --all '|' '\|')`"
}

def number [value: int]: nothing -> string {
    $value | into string --group-digits
}

def percent [value: float]: nothing -> string {
    $"($value * 100 | math round --precision 2)%"
}

def signed [value: int]: nothing -> string {
    if $value > 0 { $"+(number $value)" } else { number $value }
}

def table-md [header: list<string>, rows: list<list<string>>]: nothing -> string {
    [
        $"| ($header | str join ' | ') |"
        $"| ($header | each { '---' } | str join ' | ') |"
        ...($rows | each {|row| $"| ($row | str join ' | ') |" })
    ] | str join "\n"
}

export def function-table-md [rows: list<any>, total: int, label: string]: nothing -> string {
    table-md ['#' $label '%' 'Function'] (
        $rows | enumerate | each {|row|
            [
                ($row.index + 1 | into string)
                (number $row.item.ir)
                (percent (share $row.item.ir $total))
                (cell $row.item.function)
            ]
        }
    )
}

export def run-md [summary: record, --top: int = 50]: nothing -> string {
    [
        $"### `ccusage ($summary.command) --offline --json` at `($summary.ref)` \(($summary.sha | str substring 0..<12)\)"
        ''
        $"Total Ir: **(number $summary.total)** · fixture: ($summary.fixture)"
        ''
        $"#### Top ($top) by self Ir"
        ''
        ((function-table-md
            ($summary.self | first $top)
            $summary.total
            'Self Ir'
        ))
        ''
        $"#### Top ($top) by inclusive Ir"
        ''
        (
            (function-table-md
                ($summary.inclusive | first $top)
                $summary.total
                'Inclusive Ir'
            )
        )
        ''
    ] | str join "\n"
}

export def ranking-md [ranked: list<any>, count: int, --top: int = 50]: nothing -> string {
    table-md [
        '#'
        'Function'
        'Commands'
        'Mean self %'
        'Max self %'
        'Self Ir (sum)'
    ] (
        $ranked | first $top | enumerate | each {|row|
            [
                ($row.index + 1 | into string)
                (cell $row.item.function)
                $"($row.item.commands)/($count)"
                (percent $row.item.mean_share)
                (percent $row.item.max_share)
                (number $row.item.self_ir)
            ]
        }
    )
}

export def deltas-md [
    result: record
    base: string
    head: string
    --top: int = 50
]: nothing -> string {
    [
        $"### Total Ir, `($base)` → `($head)`"
        ''
        (
            table-md ['Command' $base $head 'Δ Ir' 'Ratio'] (
                $result.commands | each {|row|
                    [
                        (cell $row.command)
                        (number $row.base)
                        (number $row.head)
                        (signed $row.delta)
                        ($row.ratio | math round --precision 4 | into string)
                    ]
                }
            )
        )
        ''
        $"### Largest per-function self Ir changes, summed over shared commands \(top ($top)\)"
        ''
        (
            table-md ['#' 'Function' $base $head 'Δ Ir'] (
                $result.functions | first $top | enumerate | each {|row|
                    [
                        ($row.index + 1 | into string)
                        (cell $row.item.function)
                        (number $row.item.base)
                        (number $row.item.head)
                        (signed $row.item.delta)
                    ]
                }
            )
        )
        ''
    ] | str join "\n"
}
