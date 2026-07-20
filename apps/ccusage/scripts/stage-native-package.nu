#!/usr/bin/env nix
#! nix shell --inputs-from ../../.. nixpkgs#nushell --command nu
const package_dirs = {
    darwin-arm64: 'ccusage-darwin-arm64'
    darwin-x64: 'ccusage-darwin-x64'
    linux-arm64: 'ccusage-linux-arm64'
    linux-x64: 'ccusage-linux-x64'
    win32-arm64: 'ccusage-win32-arm64'
    win32-x64: 'ccusage-win32-x64'
}
def main [--platform: string, --arch: string, --binary: string] {
    let key = $"($platform)-($arch)"
    let package_dir = $package_dirs | get -o $key
    if $package_dir == null {
        error make {msg: $"Unsupported native package target: ($key)"}
    }
    let script_dir = $env.CURRENT_FILE | path dirname
    let repo_root = [$script_dir, '..', '..', '..'] | path join | path expand
    let binary_name = if $platform == 'win32' { 'ccusage.exe' } else { 'ccusage' }
    let source = $binary | path expand
    let target_dir = [$repo_root, 'packages', $package_dir, 'bin'] | path join
    let target = [$target_dir, $binary_name] | path join
    mkdir $target_dir
    cp -f $source $target
    if $platform != 'win32' {
        chmod 755 $target
    }
    if $platform == 'darwin' {
        rewrite_darwin_system_libraries $target
    }
    print $target
}
def rewrite_darwin_system_libraries [binary_path: string] {
    let linked = run-external otool '-L' $binary_path | complete
    if $linked.exit_code != 0 {
        error make {msg: $"otool failed for ($binary_path)\n($linked.stderr)"}
    }
    for line in ($linked.stdout | lines | skip 1) {
        let library = (
            $line | str trim | split row --regex '\s+' | first
        )
        if $library =~ '^/nix/store/[^/]+-libiconv-[^/]+/lib/libiconv\.2\.dylib$' {
            let rewrite = (
                run-external install_name_tool '-change' $library /usr/lib/libiconv.2.dylib $binary_path
                | complete
            )
            if $rewrite.exit_code != 0 {
                error make {msg: $"install_name_tool failed for ($library)\n($rewrite.stderr)"}
            }
        }
    }
}
