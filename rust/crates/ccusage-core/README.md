# ccusage-core

The runtime every adapter and the binary share: pricing, cost calculation,
report shaping, table output, and the date and progress helpers. Configuration
lives in `ccusage-config` and the billing-block report in the binary, because
neither has a consumer outside it.

## Owns

- `pricing.rs` — the `PricingMap`, the embedded models.dev and LiteLLM snapshots,
  the built-in rate tables, and the optional runtime fetch. models.dev repeats
  every model once per catalog that serves it, so
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

`build.rs` compacts the pinned LiteLLM snapshot into the binary. It reads
`CCUSAGE_PRICING_JSON_PATH`, which every Nix build and the dev shell set; the
`fetch-litellm-pricing` feature adds the HTTPS download that plain
`cargo build` needs on platforms Nix cannot target. It also minifies the
committed models.dev snapshot on the way in, so that file can stay indented for
reviewable regeneration diffs without shipping the indentation.

## Depends on

- `ccusage-cli`
- `ccusage-terminal`
- `jiff`
- `memchr`
- `rustc-hash`
- `serde`
- `serde_json`
- `smallvec`
- `ureq`

## Build layer

Built in the `foundation` Crane artifact layer, so a change here recompiles every adapter.
