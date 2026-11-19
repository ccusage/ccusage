# Agent Adapters

Use one crate per source. Keep source-specific discovery, parsing, token and
model mapping, and pricing in that crate. Share discovery, aggregation, dates,
pricing lifecycle, progress, and rendering through `ccusage-core` or
`ccusage-adapter-common`. Keep adapters independent of one another.

Use these modules inside each adapter:

- `paths.rs` owns environment variables, default directories, and path discovery.
- `parser.rs` parses raw records and maps tokens and models.
- `loader.rs` owns file walking, SQLite reads, deduplication, and date-filtering entry points.
- `report.rs` shapes JSON and table rows when the source needs its own output.
- `types.rs` holds types local to the adapter.

When a source normalizes or maps a model ID, pass the logged ID first as an
`Exact` candidate and use the normalized form for fuzzy matches. Preserve the
original match kind when the ID is unchanged. Use source provider information
for provider-qualified candidates.

Use `EntrySource` for loaders that already produce `Vec<LoadedEntry>` and
`UsageSource` for source-specific pipelines. Keep source identity and table policy
on the descriptor. Override report hooks only for source-specific semantics.

Share JSONL scanning when the source has stable record markers. On high-volume
paths, prefer worker-side aggregation or typed transfer to large intermediate
vectors. Keep typed read and decode errors up to the `common::units` and
`common::jsonl` boundaries. Include categories and locations in diagnostics and
omit raw log values.

Document non-obvious source paths, record shapes, and accounting rules in the
adapter README or its `src/README.md`.
