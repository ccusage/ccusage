# ccusage-zstd

Reads lines of a zstd-compressed file by decoding each block straight into the
ring the lines are sliced from, through libzstd's buffer-less streaming API.

This is the only crate allowed `unsafe` code. Keep the unsafe calls in
`src/direct.rs`, each with a `// SAFETY:` comment, and keep the differential
tests against the `zstd` crate's streaming decoder passing.

## Build layer

Built in the `foundation` Crane layer. A change here recompiles every adapter.
