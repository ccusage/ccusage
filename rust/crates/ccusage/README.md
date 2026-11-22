# ccusage

Keep the binary focused on startup and dispatch. Leave the pricing HTTP client
here to keep its TLS dependencies outside the adapter graph. Release builds set
`CCUSAGE_VERSION`.

## Build layer

The final binary builds outside the cached Crane layers.
