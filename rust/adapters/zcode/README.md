# ccusage-adapter-zcode

The ZCode adapter reads completed model-usage records from the ZCode desktop
application's local SQLite ledger and turns them into the entries used by
focused and unified reports.

## Data source

```text
$ZCODE_HOME/   # or ~/.zcode
└── cli/
    └── db/
        └── db.sqlite
```

The adapter reads `model_usage` joined to `session`. It selects identifiers,
timestamps, status, token counters, provider/model metadata, and the session
directory only. Prompt, content, rollout, and raw usage fields are not read.

The supported schema requires `model_usage.id`, `session_id`, `started_at`,
`model_id`, `status`, `input_tokens`, and `output_tokens`, plus
`session.id` and `session.directory`. `cache_creation_input_tokens`,
`cache_read_input_tokens`, `computed_total_tokens`, `provider_id`, and
`session.version` are read when present, and `--debug` lists which optional
columns were found. Compatible future versions are accepted when the required
columns remain available, while unrelated or incomplete schemas are skipped
with the missing columns named.

Connections use SQLite read-only mode, a busy timeout, and connection-local
query-only mode. The adapter does not change journal mode, so an existing
ZCode WAL remains readable while the application is running.

A non-empty `ZCODE_HOME` may contain comma-separated roots. Roots are
canonicalized and deduplicated; invalid configured roots are reported with
`--debug` and do not silently fall back to `~/.zcode`. Databases are also
deduplicated, and duplicate usage ids keep the first configured root's row.

Only rows with `status = 'completed'` are included. `started_at` is interpreted
as an epoch-millisecond timestamp and retains millisecond precision. Daily,
monthly, and session summaries use the standard ccusage report shapes.

## Token and cost mapping

ZCode's `input_tokens` includes the cache-read and cache-creation slices. The
adapter subtracts both from fresh input and reports each cache bucket
separately. Negative counters are clamped to zero, and cache buckets are
bounded by the inclusive input count. Any positive remainder in
`computed_total_tokens` supplies any residual used in reports. Pricing uses the
recorded component counts.

ZCode records no per-request cost. `display` therefore reports zero cost;
`calculate` and `auto` estimate from the shared pricing map. Raw model pricing
overrides are honored. Z.ai-qualified candidates are used for explicit Z.ai
providers and for GLM model ids from legacy rows without a provider; an unknown
custom provider is not treated as Z.ai. Z.ai cache-creation tokens remain in
their displayed bucket but are priced at the model's standard input rate because
they represent new content.
Unknown custom model ids remain at zero and use the normal missing-pricing
warning. No provider billing is inferred from a local provider UUID.

## Testing

```text
cargo test -p ccusage-adapter-zcode
```

The fixture tests cover cache normalization, timestamp precision, completed
row filtering, legacy and incompatible schemas, optional metadata, root and
database deduplication, custom-provider pricing, and focused report shapes.
