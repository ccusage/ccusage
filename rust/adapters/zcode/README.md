# ccusage-adapter-zcode

The ZCode adapter: it turns the ZCode desktop app's SQLite usage ledger into
the usage entries the reports render.

## Owns

- `loader.rs` — reading the source, dedupe, and date filtering.
- `parser.rs` — row mapping, token semantics, and model naming.
- `paths.rs` — environment variables, default directories, and file discovery.
- `report.rs` — the JSON and table shapes where they differ from the shared ones.

Anything that is not specific to this source belongs in `ccusage-core` or
`ccusage-adapter-common` instead.

## Data source

- `${ZCODE_HOME:-~/.zcode}/cli/db/db.sqlite`

The `model_usage` table records one row per model request. The adapter selects
rows with `status = 'completed'` — error and cancelled attempts record zero
tokens — and joins `session` for the project directory.

## Token semantics

- `input_tokens` **includes** the cache-read and cache-creation slices: the schema's own
  `computed_total_tokens` is `input_tokens + output_tokens` exactly, and
  the cache buckets fit within `input_tokens` for every observed row. The parser
  carves both slices out of input so the reported buckets stay additive and
  cache tokens price at their cheaper rates.
- `reasoning_tokens` and `cache_creation_input_tokens` exist in the schema but
  are zero in every observed row. Reasoning is assumed to sit inside output
  (matching how the GLM provider reports it); if a future version moves
  reasoning outside output, `computed_total_tokens` grows past the counted
  buckets and the shared total-token fallback routes the difference into the
  extra-tokens bucket instead of dropping it.

## Cost semantics

ZCode records no per-request cost. The default `auto` mode and `calculate`
estimate costs from the pricing tables; `display` reports zero because there is
no recorded cost to display. Pricing lookup tries the raw model id first, then a
provider-qualified Z.ai id and normalized aliases, so exact custom-provider
overrides and Z.ai pricing both work.

## Schema stability

The database layout is undocumented and the app updates frequently. A database
that cannot be opened or queried degrades to no entries with a `--debug` log
line, the same contract the Kilo adapter uses; the fixture tests pin the
current shape.

## Public surface

- `loader::load_entries`
- `report::report_from_rows`
- `report::summarize_entries`
- `run`

## Depends on

- `ccusage-adapter-common`
- `ccusage-core`
- `jiff`
- `serde_json`
- `sqlite`

## Build layer

Built in the `adapters` Crane artifact layer; the layer compiles all adapters in one Cargo invocation, so they build concurrently.
