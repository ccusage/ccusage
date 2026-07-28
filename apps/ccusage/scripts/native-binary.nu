# Shared helpers for the scripts that stage and validate the Rust binary
# shipped inside packages/ccusage-<platform>-<arch>.

# The file name npm expects for a Node platform identifier.
export def binary-name [platform: string]: nothing -> string {
    match $platform {
        'win32' => 'ccusage.exe'
        _ => 'ccusage'
    }
}

# The dynamic libraries `otool -L` reports for a Mach-O binary.
#
# `ok` is false when otool itself failed, so a caller can choose between
# aborting with the captured stderr and treating the binary as unusable.
export def linked-dylibs [binary: path]: nothing -> record {
    let result = run-external otool '-L' $binary | complete
    {
        ok: ($result.exit_code == 0)
        stderr: $result.stderr
        dylibs: (match $result.exit_code {
            # Only the `<dylib> (compatibility version ...)` rows are tab
            # indented. Selecting on that indent rather than skipping a fixed
            # number of leading rows keeps the unindented `<binary>
            # (architecture <arch>):` header out of the result for fat binaries,
            # which repeat that header once per architecture.
            0 => (
                $result.stdout
                | lines
                | where {|line| $line | str starts-with (char tab) }
                | each {|line| $line | str trim | split row --regex '\s+' | first }
            )
            _ => []
        })
    }
}
