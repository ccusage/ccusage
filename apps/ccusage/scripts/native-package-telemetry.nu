const telemetry_endpoint = 'https://telemetry.ccusage.com/v1/ping'

export def verify_telemetry_endpoint [binary_path: path, telemetry_enabled: bool]: nothing -> nothing {
    let endpoint_present = (
        (
            open --raw $binary_path
            | into binary
            | bytes index-of ($telemetry_endpoint | into binary)
        ) >= 0
    )
    match [$telemetry_enabled $endpoint_present] {
        [true, true] => null
        [false, false] => null
        [true, false] => (
            error make {msg: 'Native package binary is missing the telemetry endpoint'}
        )
        [false, true] => (
            error make {
                msg: 'Native package binary unexpectedly contains the telemetry endpoint'
            }
        )
    }
}
