# ccusage

The distributed binary. It stays thin on purpose: argument parsing lives in
`ccusage-cli-parser`, shared runtime behavior in `ccusage-core`, and every data
source in its own `ccusage-adapter-*` crate.

## Owns

- `main.rs` — startup, command dispatch, and the version string the release
  embeds through `CCUSAGE_VERSION`.
- `cli.rs` — the parse entry point and the config context it passes to the parser.
- `adapter/` — aliases for unified dispatch and Claude's dedicated report commands.
  Focused agent dispatch uses `ccusage-adapter-all::runner`; Claude owns blocks
  and statusline.
- `http.rs` and `http/` — the pricing refresh's HTTP client, installed into
  `ccusage-core` at startup: ureq with an ETag cache and 304 revalidation, over a
  verify-only rustls provider (TLS 1.3, X25519, RSA and ECDSA verification).
  Keeping it here keeps ureq, rustls and ring out of the crates every adapter builds.
- `ccusage-config` owns the schema generator for `apps/ccusage/config-schema.json`;
  the schema formatter and flake check reject drift.

## Depends on

- `ccusage-adapter-all`
- `ccusage-adapter-claude`
- `ccusage-cli`
- `ccusage-cli-parser`
- `ccusage-config`
- `ccusage-core`
- `ccusage-report`
- `ccusage-pricing-snapshots`
- `ring`
- `rustls`
- `rustls-webpki`
- `serde`
- `serde_json`
- `ureq`

## Build layer

Outside the Crane artifact layers: it is compiled with the final binary, so editing it leaves the cached layers untouched. Its link step is the dominant cost of a warm build, because the release
profile uses fat LTO with a single codegen unit.
