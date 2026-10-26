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

Only `model_completed` events inside a `runtime.session` record are counted. A completion's `recorded_at` value is interpreted as microseconds since the Unix epoch.

`runtime.session.metadata` records provide project and fallback model information only through `payload.record.workspace_root`, `payload.record.model_id`, and `payload.record.provider_id`. The completion's `event.model` remains primary. The initial metadata snapshot may follow the first completion and backfill earlier completions in that file; later snapshots affect subsequent completions only. A metadata provider applies only to its paired `model_id`; `unknown` and `muse` provider sentinels are ignored. These fields follow the verified prior-art shape in [TokScale PR #1349](https://github.com/junhoyeo/tokscale/pull/1349) and [PR #1355](https://github.com/junhoyeo/tokscale/pull/1355).

Muse reports `input_tokens` gross, including cache reads. ccusage prefers `cache_read_tokens`, falls back to `cached_tokens`, clamps cache reads to gross input, and reports the remainder as input. `cache_write_tokens` is reported as cache creation. `reasoning_tokens` is already part of output and is not added again. Streaming events and cumulative session totals are not counted as additional completions.

Records with both a stream ID and record ID are deduplicated within their session file path. Records without both identity fields are retained by record position so separate calls with matching token counts are not collapsed. Parent and child session files have separate deduplication scopes, so matching IDs in both files are counted independently. The on-disk format does not establish a general provider-retry identity; see the anonymised empirical excerpt and limits in [the adapter notes](https://github.com/ccusage/ccusage/issues/1694).

## Cost estimates

Muse session records do not include a USD cost. `auto` and `calculate` modes estimate cost from the recorded model and ccusage pricing data; `display` mode shows no calculated cost. Model names are retained as recorded. Muse Spark names can resolve to Meta pricing, including offline pricing where the embedded catalogue contains the model. If neither the completion nor supported session metadata supplies a model, it is kept as `unknown`; an unrecognized model is not replaced with an unrelated priced model.

## Environment variables

| Variable        | Description                                                                   |
| --------------- | ----------------------------------------------------------------------------- |
| `MUSE_DATA_DIR` | ccusage-specific comma-separated Muse data roots, each containing `sessions/` |

## Troubleshooting

If no Muse usage appears, check that `session.jsonl` files exist under `$XDG_DATA_HOME/muse/sessions/` or `$HOME/.local/share/muse/sessions/`. Muse Code's `--no-session-log` option disables the local session files ccusage reads.
