# Muse Code Data Source

ccusage reads local usage records written by Meta Muse Code. Use the focused reports to view Muse usage on its own, or include detected Muse sessions in the regular reports.

## Usage

```sh
# Daily Muse usage
ccusage muse daily

# Monthly Muse usage
ccusage muse monthly

# Muse sessions
ccusage muse session

# JSON output
ccusage muse daily --json
```

The focused reports use the shared [daily](/guide/daily-reports), [monthly](/guide/monthly-reports), and [session](/guide/session-reports) formats. See [JSON output](/guide/json-output) for the report fields.

## Data location

ccusage looks recursively for `session.jsonl` files under:

```text
${XDG_DATA_HOME:-$HOME/.local/share}/muse/sessions/
```

The same XDG and home fallback applies on macOS, Linux, and Windows. If `XDG_DATA_HOME` is unset or empty, ccusage uses `$HOME/.local/share`. Subagent logs under a session's `subagent/<child-id>/` directory are included as separate sessions, using the nearest session directory name as the session ID.

To scan one or more alternate Muse data roots, set `MUSE_DATA_DIR` to comma-separated directories that each contain a `sessions/` directory:

```sh
MUSE_DATA_DIR="$HOME/.local/share/muse,/backup/muse" ccusage muse daily
```

`MUSE_DATA_DIR` is a ccusage-specific override; it is not a verified Muse Code setting. Muse's documented replay path is described in [Meta's deterministic replay guide](https://dev.meta.ai/docs/cookbook/deterministic-replay?project_id=1661600634933790&team_id=2096920474558192). No separate macOS `Library` or Windows `AppData` session path has been verified, so ccusage does not scan those locations.

## Usage records

Only `model_completed` events inside `runtime.session` records are considered. A counted event must directly contain a non-empty `event.model` beginning with `muse-spark-`, a positive integer `recorded_at` in valid microseconds since the Unix epoch, and integer `input_tokens`, `output_tokens`, `cache_read_tokens`, and `cache_write_tokens` values. Token counts must be non-negative; explicit zero counts are valid. Missing or malformed fields, timestamp strings or fractions, and out-of-range timestamps cause that completion to be skipped. ccusage does not infer timestamp units, replace missing counts with zero, use `cached_tokens`, or fill in a missing model from metadata.

Only explicitly Meta-routed Muse Spark completions are included. An event's `provider` field is authoritative; if it is present and is not `meta`, the event is excluded. When that field is absent, a preceding `runtime.session.metadata` snapshot can establish the route only when `payload.record.provider_id` is `meta` and `payload.record.model_id` exactly matches the event's own model. Later metadata does not apply to earlier events. Metadata never supplies a missing model. `payload.record.workspace_root` may set the project path for later completions. `unknown` and `muse` provider sentinels do not establish a route.

For the explicitly Meta-routed records in scope, `cache_read_tokens` is included in gross `input_tokens`; ccusage reports input after subtracting cache reads. A record with cache reads greater than gross input is rejected. This mapping follows Meta's [prompt-caching documentation](https://dev.meta.ai/docs/prompt-caching) for the Meta Model API and is not generalized to other providers. Only `cache_write_tokens: 0` is accepted until non-zero cache-write billing semantics and rates are verified. Streaming events and cumulative session totals are not counted as completions.

Completions are deduplicated globally by the pair of non-empty opaque string IDs `stream.id` and top-level record `id`, across dates, session paths, and configured data roots. Records missing either ID are excluded. If copies with the same identity disagree on model, token counts, the original microsecond timestamp, or provider route, all copies are excluded. The available records do not establish a provider-level retry request ID, so retries written with new identities may still be counted more than once. The anonymised [issue #1694 excerpt](https://github.com/ccusage/ccusage/issues/1694) remains unchanged and is intentionally ineligible because it has no top-level record ID.

## Cost estimates

Muse session records do not include a USD cost. `auto` and `calculate` modes estimate cost from the recorded model and ccusage pricing data; `display` mode shows no calculated cost. Current official Meta pricing covers `muse-spark-1.1`, `muse-spark-1.2`, `muse-spark-1.3`, `muse-spark-1.2-contributor`, and `muse-spark-1.3-contributor`. An otherwise eligible later Muse Spark version can still contribute tokens, but receives no estimated cost and is reported as unpriced until an exact catalogue entry is available. The current Meta rates are listed in [Meta's pricing and rate-limit documentation](https://dev.meta.ai/docs/pricing-rate-limits).

## Environment variables

| Variable        | Description                                                                   |
| --------------- | ----------------------------------------------------------------------------- |
| `MUSE_DATA_DIR` | ccusage-specific comma-separated Muse data roots, each containing `sessions/` |

## Troubleshooting

If no Muse usage appears, check that `session.jsonl` files exist under `$XDG_DATA_HOME/muse/sessions/` or `$HOME/.local/share/muse/sessions/`. Muse Code's `--no-session-log` option disables the local session files ccusage reads.
