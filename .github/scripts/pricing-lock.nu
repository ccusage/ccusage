export def report [result: record]: nothing -> string {
    $result | to json --raw
}

export def --wrapped run-update [...args: string]: nothing -> nothing {
    let result = run-external ...$args | complete
    print --stderr --no-newline $result.stdout
    print --stderr --no-newline $result.stderr
    if $result.exit_code != 0 {
        error make {msg: $"Pricing update failed with exit code ($result.exit_code)"}
    }
}
