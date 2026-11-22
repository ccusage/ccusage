#!/usr/bin/env nix
#! nix shell --inputs-from ../../.. nixpkgs#nushell nixpkgs#zstd --command nu

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

def uncompressed-size [file: string]: nothing -> int {
    if ($file | str ends-with '.zst') {
        ^zstd --test --quiet $file
        ^zstd --decompress --stdout --quiet $file | length
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

def codex-rollouts [roots: list<string>]: nothing -> list<string> {
    $roots
    | each {|root|
        let subdirs = ['sessions' 'archived_sessions'] | where {|sub| ($root | path join $sub | path expand | path type) == 'dir' }
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

def main []: nothing -> nothing {
    let home = $env.HOME
    let xdg_config_home = $env | get --optional XDG_CONFIG_HOME | default ''
    let config_home = if ($xdg_config_home | str trim | is-empty) {
        $home | path join '.config'
    } else {
        $xdg_config_home
    }
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
