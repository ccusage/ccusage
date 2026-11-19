#!/usr/bin/env nix
#! nix shell --inputs-from ../../.. nixpkgs#nushell nixpkgs#zstd --command nu

# Prints the file-size distribution of the local agent logs that
# generate-large-fixture.ts reads its profiles from. Only file counts and
# size quantiles leave this script; no log content, path, or name does.

# The upper tail is sampled finely: a few huge files hold much of the
# bytes, and coarse points there would skew every generated size.
const percentiles = [
    0
    0.1
    0.25
    0.5
    0.75
    0.9
    0.95
    0.98
    0.99
    0.995
    0.999
    1
]

def env-roots [name: string, defaults: list<string>]: nothing -> list<string> {
    let value = $env | get --optional $name | default ''
    if ($value | str trim | is-empty) {
        $defaults
    } else {
        $value | split row ',' | each { str trim } | where { is-not-empty }
    } | path expand | uniq | where { path exists }
}

def files-under [roots: list<string>, patterns: list<string>]: nothing -> list<string> {
    $roots | each {|root|
        $patterns | each {|pattern| glob ($root | path join $pattern) --no-dir }
    } | flatten | flatten | uniq
}

# Codex keeps older rollouts zstd-compressed; the parser cost follows the
# decompressed bytes, so that is the size the profile records.
def uncompressed-size [file: string]: nothing -> int {
    if ($file | str ends-with '.zst') {
        # The pipeline below cannot see zstd's exit status, so a corrupt
        # archive would otherwise count as a short file.
        ^zstd --test --quiet $file
        ^zstd --decompress --stdout --quiet $file | ^wc -c | str trim | into int
    } else {
        ls $file | get 0.size | into int
    }
}

def profile [files: list<string>]: nothing -> record {
    let sizes = $files | par-each {|file| uncompressed-size $file } | sort
    let count = $sizes | length
    if $count == 0 {
        return {
            files: 0
            totalBytes: 0
            quantiles: []
        }
    }
    {
        files: $count
        totalBytes: ($sizes | math sum)
        quantiles: (
            $percentiles | each {|percentile|
                let index = ($percentile * ($count - 1)) | math round | into int
                {percentile: $percentile size: ($sizes | get $index)}
            }
        )
    }
}

# Mirrors the Codex adapter: a rollout is read once per home and path
# relative to sessions/ or archived_sessions/, ignoring .zst, preferring the
# active copy and then the plain one.
def codex-rollouts [roots: list<string>]: nothing -> list<string> {
    $roots
    | each {|root|
        let subdirs = ['sessions' 'archived_sessions'] | where {|sub| $root | path join $sub | path exists }
        # A home without either directory is scanned directly, as ccusage does.
        $subdirs | default --empty ['.'] | each {|sub|
            let dir = $root | path join $sub | path expand
            files-under [$dir] ['**/*.jsonl' '**/*.jsonl.zst']
            | sort
            | each {|file|
                let relative = $file | path relative-to $dir | str replace --regex '\.zst$' ''
                {key: $"($root)\u{0}($relative)" file: $file}
            }
        }
    }
    | flatten
    | flatten
    | uniq-by key
    | get file
}

# Every root ccusage would read is included, so several CLAUDE_CONFIG_DIR
# entries add up the way they do in a real report.
def main []: nothing -> nothing {
    let home = $env.HOME
    let xdg_config_home = $env | get --optional XDG_CONFIG_HOME | default ''
    let config_home = if ($xdg_config_home | str trim | is-empty) {
        $home | path join '.config'
    } else {
        $xdg_config_home
    }
    # Like ccusage, accept a root that names its projects/ directory.
    let claude_roots = env-roots 'CLAUDE_CONFIG_DIR' [
        ($config_home | path join 'claude')
        ($home | path join '.claude')
    ] | each {|root| if ($root | path basename) == 'projects' { $root | path dirname } else { $root } } | uniq
    let codex_roots = env-roots 'CODEX_HOME' [
        ($home | path join '.codex')
    ]
    let grok_roots = env-roots 'GROK_HOME' [
        ($home | path join '.grok')
    ]

    {
        claude: (profile (files-under $claude_roots ['projects/**/*.jsonl']))
        codex: (profile (codex-rollouts $codex_roots))
        grok: (profile (files-under $grok_roots ['sessions/**/updates.jsonl']))
    }
    | to json
    | print
}
