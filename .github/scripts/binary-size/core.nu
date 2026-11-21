use ../profile-instructions/core.nu [split-list]

# The artifacts `release.yaml` ships from Nix, on the runners and sticky disks it builds them with.
# Windows builds with cargo outside Nix, and the Intel macOS binary is a cross-build of the same
# source as arm64 on the same scarce runners, so both are left out to keep a run to minutes.
export const targets = [
    {
        target: linux-x64
        os: blacksmith-32vcpu-ubuntu-2404
        package: ccusage-release-linux-x64
        cache_scope: build-linux-x64
        symbols: true
    }
    {
        target: linux-arm64
        os: blacksmith-32vcpu-ubuntu-2404-arm
        package: ccusage-release-linux-arm64
        cache_scope: build-linux-arm64
        symbols: true
    }
    {
        target: darwin-arm64
        os: blacksmith-12vcpu-macos-26
        package: ccusage-release-darwin-arm64
        cache_scope: build-mac-arm64
        # Mach-O symbol tables carry no sizes, so `nm --size-sort` has nothing to rank.
        symbols: false
    }
]

const key_sections = {
    text: [.text __text]
    rodata: [.rodata __const]
    eh_frame: [.eh_frame __eh_frame]
}

export def select-targets [names: string]: nothing -> list<record> {
    let wanted = split-list $names
    let known = $targets | get target
    let unknown = $wanted | where $it not-in $known
    if ($unknown | is-not-empty) {
        error make {
            msg: $"Unknown target ($unknown | str join ', '); expected some of ($known | str join ', ')"
        }
    }
    match $wanted {
        [] => $targets
        _ => ($targets | where target in $wanted)
    }
}

export def parse-limit [value: string]: nothing -> any {
    match ($value | str replace --all --regex '[\s,_]' '') {
        '' => null
        $digits if ($digits =~ '^\d+$') => ($digits | into int)
        _ => { error make {msg: $"Limit '($value)' is not a whole number of bytes"} }
    }
}

export def matrix [refs: list<record>, targets: list<record>]: nothing -> list<record> {
    $refs | each {|ref|
        $targets | each {|target|
            $target | merge $ref | insert id $"($ref.slug)-($target.target)"
        }
    } | flatten
}

# Rows of `llvm-size -A`, which reads ELF and Mach-O alike. Mach-O repeats section names across
# segments, so the table keeps its order and duplicates.
export def parse-sections [output: string]: nothing -> list<record<name: string, size: int>> {
    $output
    | lines
    | parse --regex '^(?<name>\S+)\s+(?<size>\d+)\s+\d+\s*$'
    | where name != Total
    | update size { into int }
}

# The first match wins, which is `__TEXT,__const` rather than `__DATA,__const` on Mach-O.
export def key-sections [sections: list<record<name: string, size: int>>]: nothing -> record {
    $key_sections | items {|key names|
        {key: $key value: ($sections | where name in $names | get --optional 0.size)}
    } | transpose --header-row --as-record
}

def symbol-name [name: string]: nothing -> string {
    $name | str replace --regex '\.llvm\.\d+$' '' | str replace --regex '::h[0-9a-f]{16}$' ''
}

# Symbols of `llvm-nm --size-sort -S --demangle --defined-only`, summed per name once the
# per-build hash suffixes are gone, so the same function lines up across refs.
export def parse-symbols [output: string]: nothing -> list<record<name: string, size: int>> {
    $output
    | lines
    | parse --regex '^[0-9a-fA-F]+ (?<size>[0-9a-fA-F]+) \S (?<name>.+)$'
    | each {|row| {name: (symbol-name $row.name) size: ($row.size | into int --radix 16)} }
    | where size > 0
    | group-by name --to-table
    | each {|group| {name: $group.name size: ($group.items.size | math sum)} }
    | sort-by --reverse size
}

export def symbol-deltas [base: list<record<name: string, size: int>>, head: list<record<name: string, size: int>>, count: int]: nothing -> record<growths: list<any>, shrinks: list<any>> {
    let changed = $base
    | rename name base
    | join --outer ($head | rename name head) name
    | default 0 base
    | default 0 head
    | insert delta {|row| $row.head - $row.base }
    | where delta != 0
    {
        growths: (
            $changed
            | where delta > 0
            | sort-by --reverse delta
            | first $count
        )
        shrinks: (
            $changed
            | where delta < 0
            | sort-by delta
            | first $count
        )
    }
}

def delta [base, value]: nothing -> any {
    if $base == null or $value == null { null } else { $value - $base }
}

def target-report [
    target: record
    refs: list<record>
    builds: list<record>
    limit
    count: int
]: nothing -> record {

    # `each` drops null results, so a missing build stays paired with its ref instead.
    let found = $refs | each {|ref|
        {ref: $ref build: ($builds | where ref == $ref.ref and target == $target.target | get --optional 0)}
    }
    let base = $found | first | get build
    let rows = $found | each {|pair|
        let ref = $pair.ref
        let build = $pair.build
        match $build {
            null => {ref: $ref.ref sha: $ref.sha built: false}
            _ => {
                ref: $ref.ref
                sha: $ref.sha
                built: true
                bytes: $build.bytes
                delta: (delta $base.bytes? $build.bytes)
                sections: $build.sections
                section_deltas: (
                    $build.sections | items {|key value| {key: $key value: (delta ($base.sections? | get --optional $key) $value)} }
                    | transpose --header-row --as-record
                )
                within_limit: (if $limit == null { null } else { $build.bytes <= $limit })
            }
        }
    }
    let symbols = match $base {
        null => []
        _ if ($base.symbols? == null) => []
        _ => (
            $found | skip 1 | get build | where {|build| $build != null and $build.symbols? != null }
            | each {|build|
                symbol-deltas $base.symbols $build.symbols $count
                | insert base $base.ref
                | insert head $build.ref
            }
        )
    }
    {
        target: $target.target
        package: $target.package
        rows: $rows
        symbols: $symbols
    }
}

export def report [
    refs: list<record>
    targets: list<record>
    builds: list<record>
    limit
    --count: int = 20
]: nothing -> record {
    {
        base: ($refs | first | get ref)
        limit: $limit
        refs: $refs
        targets: (
            $targets
            | each {|target| target-report $target $refs $builds $limit $count }
        )
    }
}

def bytes-cell [value]: nothing -> string {
    if $value == null {
        'n/a'
    } else {
        $value | into string --group-digits
    }
}

def delta-cell [value]: nothing -> string {
    match $value {
        null => ''
        0 => '±0'
        $n if $n > 0 => $"+($n | into string --group-digits)"
        $n => ($n | into string --group-digits)
    }
}

def percent-cell [base, value]: nothing -> string {
    if $base == null or $value == null or $base == 0 {
        ''
    } else {
        let percent = ($value - $base) * 100 / $base
        let text = $percent | into string --decimals 2
        if $percent > 0 { $"+($text)%" } else { $"($text)%" }
    }
}

def section-cell [row: record, key: string]: nothing -> string {
    let value = $row.sections | get $key
    let change = $row.section_deltas | get $key
    match $change {
        null | 0 => (bytes-cell $value)
        _ => $"(bytes-cell $value) \((delta-cell $change)\)"
    }
}

def short [sha: string]: nothing -> string {
    $sha | str substring 0..<10
}

def symbol-cell [name: string]: nothing -> string {
    let clipped = if ($name | str length) > 140 { $"($name | str substring 0..<139)…" } else { $name }
    $"`($clipped | str replace --all '`' "'" | str replace --all '|' '\|')`"
}

def symbol-table [kind: string, rows: list<any>]: nothing -> list<string> {
    match $rows {
        [] => [$"**No ($kind)**" '']
        _ => [
            $"**Top ($rows | length) ($kind)**"
            ''
            '| Δ bytes | Base | Head | Symbol |'
            '| ---: | ---: | ---: | --- |'
            ...(
                $rows | each {|row|
                    $"| (delta-cell $row.delta) | (bytes-cell $row.base) | (bytes-cell $row.head) | (symbol-cell $row.name) |"
                }
            )
            ''
        ]
    }
}

def target-md [target: record, limit]: nothing -> list<string> {
    let base = $target.rows | first
    let limit_header = if $limit == null { '' } else { ' Limit |' }
    let limit_rule = if $limit == null { '' } else { ' :---: |' }
    let rows = $target.rows | each {|row|
        if not $row.built {
            let filler = if $limit == null { 5 } else { 6 }
            $"| `($row.ref)` | `(short $row.sha)` | build failed |(1..$filler | each { ' |' } | str join)"
        } else {
            let check = match $row.within_limit {
                null => ''
                true => ' ✅ |'
                false => ' ❌ |'
            }
            [
                $"`($row.ref)`"
                $"`(short $row.sha)`"
                (bytes-cell $row.bytes)
                (if $row.ref == $base.ref { 'base' } else { delta-cell $row.delta })
                (if $row.ref == $base.ref { '' } else { percent-cell $base.bytes? $row.bytes })
                (section-cell $row text)
                (section-cell $row rodata)
                (section-cell $row eh_frame)
            ] | str join ' | ' | $"| ($in) |($check)"
        }
    }
    let symbols = $target.symbols | each {|pair|
        [
            ''
            $"<details><summary>Symbols <code>($pair.base)</code> → <code>($pair.head)</code></summary>"
            ''
            ...(symbol-table growths $pair.growths)
            ...(symbol-table shrinks $pair.shrinks)
            '</details>'
        ]
    } | flatten
    [
        $"## ($target.target) \(`($target.package)`\)"
        ''
        $"| Ref | SHA | Bytes | Δ bytes | Δ % | text | rodata | eh_frame |($limit_header)"
        $"| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |($limit_rule)"
        ...$rows
        ...$symbols
        ''
    ]
}

export def report-md [report: record]: nothing -> string {
    let limit = match $report.limit {
        null => []
        $bytes => [$"Limit: ($bytes | into string --group-digits) bytes per binary." '']
    }
    [
        '# Release binary sizes'
        ''
        $"Deltas are against `($report.base)`. Section sizes come from `llvm-size -A`; on macOS, text, rodata, and eh_frame are `__text`, `__TEXT,__const`, and `__eh_frame`."
        ''
        ...$limit
        ...(
            $report.targets
            | each {|target| target-md $target $report.limit }
            | flatten
        )
    ] | str join "\n"
}

export def over-limit [report: record]: nothing -> list<string> {
    $report.targets | each {|target|
        $target.rows | where {|row| $row.within_limit? == false } | each {|row| $"($target.target) at ($row.ref)" }
    } | flatten
}
