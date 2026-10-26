# ccusage-adapter-muse

The Muse Code adapter reads eligible completed model usage from Muse session JSONL files.

## Fixture provenance

`fixtures/issue-1694-model-completed.jsonl` is an anonymised transcription of the representative Linux Muse Code 1.0.3 record quoted in [ccusage issue #1694](https://github.com/ccusage/ccusage/issues/1694). The issue abbreviated `stream.id`; this fixture keeps the visible prefix and replaces the remainder with `[redacted]`. The excerpt has no top-level record `id` or explicit Meta route, so it remains unchanged but is intentionally excluded from usage and report aggregation.

`fixtures/strict-eligibility-synthetic.jsonl` contains one fully eligible synthetic completion with stable IDs and four incomplete rows that must be skipped. Its control uses gross input 27,731, cache reads 27,121, output 20, and zero cache writes; the resulting net input is 610. The synthetic identity and route are test scaffolding, not observed Muse output. `fixtures/report-synthetic-completions.jsonl` supplies two more complete, explicitly Meta-routed records for report totals and date grouping.

`fixtures/metadata-timeline-synthetic.jsonl` tests forward-only metadata routing and project paths. Its records and values are synthetic. The metadata field paths follow prior-art examples in [TokScale PR #1349](https://github.com/junhoyeo/tokscale/pull/1349) and [PR #1355](https://github.com/junhoyeo/tokscale/pull/1355); those examples do not establish that later metadata applies to earlier completions. The `subagent-*-synthetic.jsonl` files use different stream IDs to exercise separate parent and child identities.

## Eligibility and deduplication

Only `model_completed` records with a non-empty direct `event.model` beginning `muse-spark-`, a positive in-range integer microsecond timestamp, all four non-negative integer token counters, and `cache_write_tokens: 0` can count. Explicit zero token counts are allowed. Missing, malformed, or contradictory records are skipped. The adapter does not use `cached_tokens`, fill absent token counts with zero, or backfill the model from metadata.

An event-level `provider` is authoritative. Without that field, only a preceding metadata record whose `provider_id` is `meta` and whose `model_id` exactly matches the completion model can establish the route. Later metadata never changes earlier records. Cache reads are subtracted from gross input only for explicitly Meta-routed records in scope, following Meta's [prompt-caching documentation](https://dev.meta.ai/docs/prompt-caching) for the Meta Model API. A cache-read count greater than gross input is rejected; non-zero cache writes remain excluded because their tariff is unverified.

The dedupe key is the opaque pair `(stream.id, record.id)` across all configured roots, dates, and paths. Records missing either ID are excluded. If copies with the same key disagree on model, counters, original timestamp, or provider route, every copy is excluded. No documented provider retry ID is established; a retry with a new record identity may still count twice.

Meta currently publishes pricing for the five Muse Spark IDs covered by the adapter tests: `muse-spark-1.1`, `muse-spark-1.2`, `muse-spark-1.3`, `muse-spark-1.2-contributor`, and `muse-spark-1.3-contributor`. See [Meta pricing and rate limits](https://dev.meta.ai/docs/pricing-rate-limits). Other eligible versions retain their token totals but have no estimated cost until an exact catalogue rate is available.

## Source limits

Discovery follows the documented XDG/home path on each platform. No separate macOS or Windows session root, or native Muse data-directory override, has been verified. `MUSE_DATA_DIR` is a ccusage-specific override. A discovered file read failure aborts the load so reports cannot silently use partial file totals; malformed individual JSONL records are skipped.
