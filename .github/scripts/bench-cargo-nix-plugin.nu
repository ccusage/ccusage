#!/usr/bin/env nix
#! nix shell --inputs-from ../.. nixpkgs#nushell nixpkgs#git --command nu

# Compare Crane against the experimental cargo-nix-plugin build of ccusage:
# one build from whatever store the job restored (with the shared toolchain
# already fetched), then one rebuild after each
# edit to an adapter, the shared core, and the binary crate. Every edit is
# reverted before the next scenario so both builders see the same source.
def main [
    --system: string # Nix system attribute, e.g. x86_64-linux
    --summary: path # Markdown file to append the result table to
] {
    let rev = open --raw flake.lock | from json | get nodes.cargo-nix-plugin.locked.rev
    let plugin_build = timeit {
        ^nix build --out-link cargo-nix-plugin $"github:anthropics/cargo-nix-plugin/($rev)#cargo-nix-plugin"
    }
    let plugin_files = pwd | path join cargo-nix-plugin lib nix plugins

    ^nix build --no-link $".#legacyPackages.($system).cnp.sharedInputs"

    let targets = {
        crane: [$".#packages.($system).ccusage"]
        plugin: [
            --option
            plugin-files
            $plugin_files
            $".#legacyPackages.($system).cnp.ccusage"
        ]
    }

    let scenarios = [
        {name: 'restored store', file: null}
        {name: 'edit adapters/amp', file: 'rust/adapters/amp/src/lib.rs'}
        {name: 'edit crates/ccusage-core', file: 'rust/crates/ccusage-core/src/lib.rs'}
        {name: 'edit crates/ccusage (bin)', file: 'rust/crates/ccusage/src/main.rs'}
    ]

    let rows = $scenarios | each {|scenario|
        if $scenario.file != null {
            $"\n// bench edit (date now | format date '%s%f')\n" | save --append $scenario.file
        }
        print $"::group::($scenario.name)"
        let crane = timeit { ^nix build --no-link --print-build-logs ...$targets.crane }
        let plugin = timeit { ^nix build --no-link --print-build-logs ...$targets.plugin }
        print '::endgroup::'
        if $scenario.file != null {
            ^git checkout -- $scenario.file
        }
        {scenario: $scenario.name, crane: $crane, 'cargo-nix-plugin': $plugin}
    }

    let table = $rows | to md
    print $"plugin build: ($plugin_build)"
    print $table
    $"### ($system)\n\nplugin build: ($plugin_build)\n\n($table)\n\n" | save --append $summary
}
