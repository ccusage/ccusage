#!/usr/bin/env nix
#! nix shell --inputs-from ../../.. nixpkgs#nushell --command nu

use ./native-binary.nu [binary-name, linked-dylibs]

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
    let package_dir = (match ($package_dirs | get --optional $key) {
        null => (error make {msg: $"Unsupported native package target: ($key)"})
        $dir => $dir
    })
    let script_dir = $env.CURRENT_FILE | path dirname
    let repo_root = [$script_dir, '..', '..', '..'] | path join | path expand
    let source = $binary | path expand
    let target_dir = [$repo_root, 'packages', $package_dir, 'bin'] | path join
    let target = [
        $target_dir
        (binary-name $platform)
    ] | path join
    mkdir $target_dir
    cp -f $source $target
    finalize_target $platform $target
    print $target
}
def finalize_target [platform: string, target: path] {
    match $platform {
        'win32' => null
        'darwin' => {
            chmod 755 $target
            rewrite_darwin_system_libraries $target
        }
        _ => (chmod 755 $target)
    }
}
def rewrite_darwin_system_libraries [binary_path: string] {
    let linked = (linked-dylibs $binary_path)
    if not $linked.ok {
        error make {msg: $"otool failed for ($binary_path)\n($linked.stderr)"}
    }
    let failed_rewrite = (
        $linked.dylibs
        | where {|library| $library =~ '^/nix/store/[^/]+-libiconv-[^/]+/lib/libiconv\.2\.dylib$' }
        | each {|library|
            {library: $library, rewrite: (run-external install_name_tool '-change' $library /usr/lib/libiconv.2.dylib $binary_path | complete)}
        }
        | where {|attempt| $attempt.rewrite.exit_code != 0 }
        | get --optional 0
    )
    match $failed_rewrite {
        null => null
        $attempt => (
            error make {msg: $"install_name_tool failed for ($attempt.library)\n($attempt.rewrite.stderr)"}
        )
    }
}
