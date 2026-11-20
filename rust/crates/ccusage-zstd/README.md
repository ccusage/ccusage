# ccusage-zstd

Reads lines of a zstd-compressed file by decoding each block straight into the
ring the lines are sliced from, through libzstd's buffer-less streaming API.

This is the only crate allowed `unsafe` code. Keep the unsafe calls in
`src/direct.rs`, each with a `// SAFETY:` comment, and keep the differential
tests against the `zstd` crate's streaming decoder passing.

## Fuzzing

`tests/fuzz.rs` runs a small randomized comparison with every `cargo test`.
Widen it with `FUZZ_ITERS`, `FUZZ_SEED` and `FUZZ_RETRY_TRIALS`, and set
`FUZZ_FRESH=1` to decode each stream on a fresh thread without recycled buffers:

```sh
FUZZ_ITERS=300 FUZZ_SEED=22 direnv exec . cargo test --release \
  --manifest-path rust/Cargo.toml -p ccusage-zstd --test fuzz -- --nocapture
```

## AddressSanitizer

Run the crate tests with both the Rust code and libzstd's C sources
instrumented. The dev shell sets `RUSTFLAGS`, so extend it rather than replace
it; clang's stack-use-after-return instrumentation is off because Rust's ASan
runtime does not provide it.

```sh
direnv exec . sh -c 'RUSTC_BOOTSTRAP=1 RUSTFLAGS="$RUSTFLAGS -Zsanitizer=address" \
  CFLAGS="-fsanitize=address -fsanitize-address-use-after-return=never -fno-omit-frame-pointer" \
  CARGO_TARGET_DIR=rust/target/asan cargo test --manifest-path rust/Cargo.toml \
  -p ccusage-zstd --target "$(rustc -vV | sed -n "s/^host: //p")"'
```

## Build layer

Built in the `foundation` Crane layer. A change here recompiles every adapter.
