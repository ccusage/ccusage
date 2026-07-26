# ccusage-core

The runtime every adapter and the binary share: pricing, cost calculation,
report shaping, table output, configuration, and the date and progress helpers.

## Owns

- `pricing.rs` — the `PricingMap`, the embedded models.dev and LiteLLM snapshots,
  the built-in rate tables, and the optional runtime fetch.
- `cost.rs` — cost calculation and missing-pricing detection.
- `summary.rs`, `agent_report.rs`, `output.rs` — row aggregation, period labels,
  JSON shaping, and table rendering.
- `config.rs`, `config_schema.rs` — `ccusage.json` loading, validation, and the
  JSON Schema the editor integration uses.
- `blocks.rs` — 5-hour billing blocks and burn rate.
- `date_utils.rs`, `fast.rs`, `home.rs`, `path_utils.rs`, `utils.rs` — timestamp
  parsing, byte-level line scanning, and small shared helpers.
- `progress.rs` — the load progress indicator.
- `types.rs`, `CliError`, and the `Result` alias every crate returns.

`build.rs` compacts the pinned LiteLLM snapshot into the binary. It reads
`CCUSAGE_PRICING_JSON_PATH`, which every Nix build and the dev shell set; the
`fetch-litellm-pricing` feature adds the HTTPS download that plain
`cargo build` needs on platforms Nix cannot target.

## Depends on

- `ccusage-cli`
- `ccusage-terminal`
- `jiff`
- `memchr`
- `rustc-hash`
- `schemars`
- `serde`
- `serde_json`
- `smallvec`
- `ureq`

## Build layer

Built in the `foundation` Crane artifact layer, so a change here recompiles every adapter.
