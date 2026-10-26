# ccusage-adapter-muse

The Muse Code adapter reads completed model usage from Muse session JSONL files.

## Fixture provenance

`fixtures/issue-1694-model-completed.jsonl` is an anonymised transcription of the representative Linux Muse Code 1.0.3 record quoted in [ccusage issue #1694](https://github.com/ccusage/ccusage/issues/1694). The issue abbreviated the `stream.id`; this fixture keeps the visible prefix and replaces the remainder with `[redacted]`. The quoted excerpt did not include the record-level `id`, so it is intentionally absent here.

Other parser and loader edge cases are synthetic test records. They exercise invalid and zero usage, cache aliases, duplicate durable records, and distinct calls with identical token counts; they are not presented as observed Muse output. `fixtures/metadata-timeline-synthetic.jsonl` uses the metadata field paths confirmed by [TokScale PR #1349](https://github.com/junhoyeo/tokscale/pull/1349) and [PR #1355](https://github.com/junhoyeo/tokscale/pull/1355), but its values and event sequence are synthetic. The two `subagent-*-synthetic.jsonl` files exercise same-identity records in separate parent and child session files.

Muse session metadata is read only from `payload.record.workspace_root`, `payload.record.model_id`, and `payload.record.provider_id` in `runtime.session.metadata` records. The completion's `event.model` remains primary. The initial metadata snapshot can backfill completions that precede it in the file; later snapshots apply to subsequent completions only. A metadata provider applies only when its `model_id` matches the completion model, and `unknown`/`muse` provider sentinels are ignored.

`fixtures/report-synthetic-completions.jsonl` contains synthetic `model_completed` records used with the observed excerpt to exercise multiple daily, monthly, and session rows. Its envelope and usage field names follow the documented completion shape, but its timestamps, IDs, models, and token counts are test data.

## Source limits

Discovery follows the documented XDG/home path on each platform. No separate macOS or Windows session root, or native Muse data-directory override, has been verified. `MUSE_DATA_DIR` is a ccusage-specific override.

The adapter counts durable `model_completed` records, deduplicating records only when both `stream.id` and record `id` are present. Records without that identity use their session path and record position as a stable, non-collapsing fallback. Each `session.jsonl` uses its nearest session directory as the session ID, so a subagent appears as its own session row; its path also gives it a separate deduplication scope. There is no verified disk contract for provider retries or partial stream usage; cumulative session-view counters are not added to completion records.
