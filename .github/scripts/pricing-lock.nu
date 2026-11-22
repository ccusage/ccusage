export def report [result: record]: nothing -> string {
    $result | to json --raw
}

export def --wrapped run-update [...args: string]: nothing -> nothing {
    ^sh -c 'exec "$@" >&2' pricing-update ...$args
}
