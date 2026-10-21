# ccusage-core

The runtime every adapter and the binary share: pricing, cost calculation,
report shaping, table output, and the date and progress helpers. Configuration
lives in `ccusage-config` and the billing-block report in the binary, because
neither has a consumer outside it.

## Owns

- `pricing.rs` — the `PricingMap`, the loaders for the embedded models.dev and
  LiteLLM snapshots, the built-in rate tables, and the optional runtime fetch.
  models.dev repeats every model once per catalog that serves it, so
  `models-dev-catalog-rules.json` carries what a live `api.json` response does
  not say: which catalogs author models, which only resell them, which model ids
  and tiers the authored catalog carries, and which models are priced per asset
  rather than per token. Both the snapshot and the runtime fetch apply it, so
  they cannot disagree about where a rate came from or which ids the fuzzy
  lookup may answer with.
- `cost.rs` — cost calculation and missing-pricing detection.
- `summary.rs`, `agent_report.rs`, `output.rs` — row aggregation, period labels,
  JSON shaping, and table rendering.
- `date_utils.rs`, `fast.rs`, `home.rs`, `path_utils.rs`, `utils.rs` — timestamp
  parsing, byte-level line scanning, and small shared helpers.
- `progress.rs` — the load progress indicator.
- `types.rs`, `CliError`, and the `Result` alias every crate returns.

The snapshot bytes themselves live in `ccusage-pricing-snapshots`, which only
the binary depends on, so the scheduled snapshot refreshes do not recompile
this crate or any adapter. The binary installs them at startup with
`pricing::set_embedded_snapshots`, and the runtime inflates each one once, on
first use. Tests enable the `test-snapshots` feature from `[dev-dependencies]`
instead, which falls back to that crate when nothing was installed.

## Depends on

- `ccusage-cli`
- `ccusage-terminal`
- `jiff`
- `memchr`
- `miniz_oxide`
- `rustc-hash`
- `serde`
- `serde_json`
- `smallvec`
- `ccusage-pricing-snapshots` — optional, behind the test-only `test-snapshots`
  feature

## Build layer

Built in the `foundation` Crane artifact layer, so a change here recompiles every adapter.
