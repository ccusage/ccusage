# Codex Speed Inference Plan

## Context

Codex logs do not currently record an explicit fast-mode flag per turn. The
current `--speed auto` behavior reads the current `~/.codex/config.toml`
`service_tier` and applies one speed choice to all historical Codex usage. That
means historical cost can change when the current Codex config changes.

Recent log checks showed that raw TPS signals exist, but the denominator matters.
Simple time between `token_count` events is too noisy because replayed history,
sub-agent activity, tool calls, and tool output can distort the interval.

Ground-truth notes from local sessions:

- Weekdays in the previous week were mostly fast mode.
- `kindle-alpha` should generally be treated as fast-mode usage.
- For today's two sessions, the smaller session was fast and the larger session
  was not fast, so raw turn-level `output_tokens / duration_ms` can classify in
  the wrong direction.

## Goals

- Preserve existing `--speed auto` behavior for compatibility.
- Add an opt-in inference mode instead of silently changing auto pricing.
- Infer speed per turn, not per session or per report bucket.
- Keep standard and fast usage in separate cost buckets so one multiplier is not
  applied to a mixed report.
- Avoid overcharging uncertain turns.

## Proposed CLI Behavior

Add a new Codex speed option:

```sh
ccusage codex daily --speed infer
ccusage codex monthly --speed infer
ccusage codex session --speed infer
```

Speed modes:

- `--speed auto`: keep current behavior. Read Codex config and apply the resolved
  speed globally.
- `--speed standard`: force all Codex usage to standard pricing.
- `--speed fast`: force all Codex usage to fast pricing.
- `--speed infer`: infer fast/standard/unknown per turn from log timing.

## Inference Strategy

Use `--speed infer` only.

Do not use raw time between `token_count` checkpoints as the primary metric.
It is useful for diagnostics, but too noisy for pricing.

Prefer a model-output interval:

1. Track turn boundaries from `task_started`, `user_message`, and `turn_context`.
2. Track model-output events such as `response_item` with `message` or
   `reasoning` payloads.
3. Exclude tool call and tool output time from the denominator where possible.
4. Compute TPS from output tokens over the model-output interval.
5. Mark tiny, short, replayed, or structurally ambiguous samples as `unknown`.

Initial buckets:

- `fast`: strong fast signal, or `kindle-alpha` fast prior.
- `standard`: strong standard signal.
- `unknown`: borderline, too few valid steps, too few output tokens, missing
  timing events, or replay-like logs.

Initial fallback:

- Treat `unknown` as standard for cost calculation.

This is conservative and avoids bringing back the core bug where current config
changes historical cost.

## Model Prior

For `--speed infer`, treat `kindle-alpha` as fast by default. Implement this in a
small helper so it is easy to adjust or override later.

Do not apply this prior to `--speed auto`, `--speed standard`, or `--speed fast`.

## Data Model Changes

The current `CodexModelUsage` stores only one token aggregate per model. That is
not enough once fast and standard usage can be mixed in the same day, month, or
session.

Add speed-aware buckets, for example:

```text
CodexModelUsage
  standard
  fast
  unknown
```

Each bucket should carry the same token fields currently stored on
`CodexModelUsage`:

- input tokens
- cached input tokens
- output tokens
- reasoning output tokens
- total tokens

Cost calculation should price each bucket separately:

- standard bucket: normal pricing
- fast bucket: pricing multiplied by `fastMultiplier`
- unknown bucket: standard pricing

## Parser Changes

Extend Codex parsing to collect timing metadata alongside token usage.

Each `CodexTokenUsageEvent` should have enough information for inference, such
as:

- turn id, if available
- model-output interval duration
- output tokens for the inferred interval
- inference confidence or bucket

If timing data is unavailable, keep the event usable for token accounting and
mark the speed bucket as `unknown`.

## Tests

Use minimal JSONL fixtures. Do not include real prompt text or assistant output.

Required regression cases:

- `--speed auto` remains config-based and does not use TPS inference.
- `--speed infer` separates standard and fast buckets within the same report
  period.
- `kindle-alpha` is fast under `infer`.
- Unknown or borderline turns fall back to standard pricing.
- Today's smaller fast-like session is not misclassified as standard because of
  overhead.
- Today's larger non-fast session is not misclassified as fast by raw
  `output_tokens / duration_ms`.
- Reports apply `fastMultiplier` only to fast buckets.
- `ccusage daily` all-agent reports use the same Codex inferred bucket costs as
  `ccusage codex daily`.

## Documentation Updates

If implemented, update:

- root `README.md`
- `apps/ccusage/README.md`
- `docs/guide/codex/index.md`
- relevant all-report examples if they mention Codex speed pricing
- CLI help snapshots and config schema artifacts

Docs should clearly state:

- Codex logs do not expose an authoritative fast flag.
- `auto` is config-based.
- `infer` is timing-based and heuristic.
- `infer` is conservative for unknown turns.

## Rollout

1. Add `CodexSpeed::Infer` and CLI/config/schema support.
2. Add failing tests for mixed standard/fast bucket pricing.
3. Add parser timing extraction with minimal fixtures.
4. Add inference helper and conservative bucket assignment.
5. Split Codex aggregation into speed-aware buckets.
6. Update Codex-focused reports.
7. Update all-agent reports.
8. Update docs and generated artifacts.
9. Run focused Rust tests, then `just fmt` and the relevant repo checks.
