# GitHub Copilot CLI Data Source (Beta)

> GitHub Copilot CLI support is experimental. The adapter reads local session-state and OpenTelemetry JSONL files.

ccusage can read GitHub Copilot CLI session-state and OpenTelemetry files as supported local data sources. It uses the same reporting experience as the rest of ccusage: responsive tables, JSON output, LiteLLM-based pricing, cache token accounting, and all-source aggregation.

## Focused Views

::: code-group

```bash [bunx (Recommended)]
bunx ccusage copilot --help
```

```bash [npx]
npx ccusage@latest copilot --help
```

```bash [pnpm]
pnpm dlx ccusage copilot --help
```

:::

## Data Source

The CLI reads Copilot session-state usage-record, shutdown, and usage-checkpoint events from `${COPILOT_HOME:-~/.copilot}/session-state/*/events.jsonl` by default. It also reads OpenTelemetry JSONL files recursively from `${COPILOT_HOME:-~/.copilot}/otel/**/*.jsonl` and includes the single explicit file pointed to by `COPILOT_OTEL_FILE_EXPORTER_PATH`. Set `COPILOT_HOME` to the Copilot data root when the default `~/.copilot` directory has been relocated.

Session-state files do not require OpenTelemetry configuration. Shutdown usage is cumulative per canonical session/model pair, and resumed sessions emit another shutdown snapshot. ccusage reports the first snapshot as-is and each later snapshot as the difference from its predecessor, so usage follows the resume dates while totals stay unchanged. For date-bounded reports, snapshots before `--since` remain available as subtraction baselines, and snapshots after `--until` are excluded. When both sources contain the same session/model pair, the session-state usage is used and matching OpenTelemetry rows are suppressed only when their timestamps are at or before the latest raw shutdown visible through `--until`; rows emitted after that timestamp by a resumed session are retained. A Copilot process restores the per-model breakdown of the session's last shutdown, so the calls of a process that ended without a shutdown (killed, crashed, or restarted by the app) are missing from every later one. When the OpenTelemetry rows of a pair outnumber the requests its shutdowns report, counted from the first shutdown interval that has OpenTelemetry rows, ccusage reports those rows instead of the shutdown intervals from that interval on. This comparison covers every shutdown, so a date-bounded report shows the same usage per day as an unbounded one. OpenTelemetry rows for other session/model pairs remain available. OpenTelemetry rows are matched across all OpenTelemetry files at once, so splitting the same rows between files does not change the report. Chat spans that the CLI exports twice, with the response ID and usage of the same call, are counted once, in the same file or in different files. A span or log record that appears more than once, in one file or several (such as a copied or rotated export), is counted once when it carries a trace ID with a span ID (or, for agent-turn logs, a turn index); rows without them cannot be matched across files and count in each file.

Current Copilot CLI versions write a `session.usage_record` event for every model call, often with no `session.shutdown` at all. ccusage reports each record as one call at its own timestamp, from `data.usage.model` and the `data.usage` token fields, counting each `accounting` identity (`sourceSessionId` and `sequence`) once across all session files. When the same record appears in several sessions' files, ccusage attributes the call to the session named by `sourceSessionId`, or else to the lowest session ID. A record without a `sourceSessionId` or a positive `sequence` is deduplicated by its event `id` within its own file. A record whose token fields are all zero still counts as one request. A shutdown's `modelMetrics` total every call before it, including the ones usage records already report, so in a session with usage records ccusage keeps only the shutdowns written before the first record. A shutdown or checkpoint with `usageAccountingWatermarks` is treated the same way even when its session's file has no usage record: the watermarks mean its totals include calls whose records are stored with the session they belong to. Each usage record replaces at most one OpenTelemetry row of the same call and model. A service request ID names one request, so every record first takes the row whose `github.copilot.service_request_id` matches its `serviceRequestId`. VS Code reports one response ID on every call of a turn, so records still without a row then take, in time order, the nearest remaining row whose `gen_ai.response.id` matches their `apiCallId`. Record matching removes no other OpenTelemetry row. A row with neither `gen_ai.response.id` nor `github.copilot.service_request_id` can never be matched, so a call that has both a usage record and such a row is counted twice. Rows that no record replaces remain subject to the shutdown reconciliation above for the shutdowns kept before the first record. Sessions without usage records use their shutdowns as described above.

For shutdown token usage, for each `data.modelMetrics.<model>` entry, ccusage reads its `usage` fields and uses any positive `requests.count` for `messageCount`; each retained OpenTelemetry usage row contributes one message. `requests.cost`, like a usage record's `data.usage.cost`, is a premium-request multiplier rather than dollars, so ccusage ignores it. In session-state data, usage records included, `inputTokens` includes both cache buckets, so ccusage derives uncached input as `max(inputTokens - cacheReadTokens - cacheWriteTokens, 0)` before populating `inputTokens`; cache reads and cache writes are then reported separately. OpenTelemetry `gen_ai.usage.input_tokens` includes both cache buckets as well and is normalized the same way. `reasoningTokens` is a subset of `outputTokens`, so it is not added again to output, total tokens, or cost. Copilot model IDs with `-1m` or `-1m-internal` suffixes, such as `claude-opus-4.6-1m`, are normalized to their priced model name for pricing and source deduplication.

For example, a `claude-opus-4.7` shutdown with `inputTokens=100`, `outputTokens=50`, `cacheReadTokens=10`, and `cacheWriteTokens=20` is reported as 70 input tokens, 50 output tokens, 20 cache-creation tokens, and 10 cache-read tokens. With the embedded Opus pricing, its calculated cost is exactly `$0.00173`.

An OpenTelemetry record can name reasoning outside its output count. ccusage prices that reasoning at the output rate up to the difference between the recorded total and the known token components. Any remaining difference contributes to the reported token total only.

Copilot also records what it billed, in nano-AIU: a usage record's `data.usage.copilotUsage.totalNanoAiu` for its call, and a shutdown's `data.modelMetrics.<model>.totalNanoAiu` for every call of that model so far. One AIU is one GitHub AI credit, billed at $0.01. In the default `auto` cost mode, ccusage reports these recorded credits as the cost: per call for a usage record, and, for a shutdown interval, the credits it adds over its predecessor, following the snapshot intervals above. An interval whose credits grow without new tokens is still reported, so its credits are not lost. A usage record without `copilotUsage.totalNanoAiu`, a shutdown interval where either snapshot lacks the model's `totalNanoAiu`, and every OpenTelemetry row, which carries no credits, are priced from their tokens instead. `--mode calculate` always prices tokens, and `--mode display` shows the recorded cost, which is $0 for rows without one.

A session-wide `totalNanoAiu` on shutdowns and usage checkpoints records the credits the whole session used. ccusage compares that total with the credits already reported through shutdown breakdowns and verified usage receipts. A residual that stays unexplained at later snapshots appears as a cost-only `unknown` row with zero tokens, dated at the first snapshot where it persists. An OpenTelemetry span already covered by a shutdown or usage receipt is accounted for by that source. When an unmatched span could overlap the residual, or the receipt sequence or recorded credits cannot be verified, ccusage leaves the residual unreported. Credit reconciliation requires a checkpoint and complete shutdown credit totals. It uses the full session history before applying date filters. In `--mode calculate`, the `unknown` rows show $0.

Enable these variables before starting or resuming a Copilot CLI session when you want OTel data. Sessions that ran without OpenTelemetry file export remain readable from their session-state files.

```bash
export COPILOT_HOME="$HOME/.copilot"
export COPILOT_OTEL_ENABLED=true
export COPILOT_OTEL_EXPORTER_TYPE=file
mkdir -p "$COPILOT_HOME/otel"
export COPILOT_OTEL_FILE_EXPORTER_PATH="$COPILOT_HOME/otel/copilot-otel-$(date +%Y%m%d-%H%M%S).jsonl"
```

```text
${COPILOT_HOME:-~/.copilot}/
├── session-state/
│   └── <session-id>/
│       └── events.jsonl
└── otel/
    └── *.jsonl
```

## Report Views

| Focused view              | Description                        | See also                                |
| ------------------------- | ---------------------------------- | --------------------------------------- |
| `ccusage copilot daily`   | Aggregate usage by date            | [Daily Usage](/guide/daily-reports)     |
| `ccusage copilot weekly`  | Aggregate usage by week            | [Weekly Usage](/guide/weekly-reports)   |
| `ccusage copilot monthly` | Aggregate usage by month           | [Monthly Usage](/guide/monthly-reports) |
| `ccusage copilot session` | Group usage by Copilot session IDs | [Session Usage](/guide/session-reports) |

These views support `--json` for structured output, `--compact` for narrow terminals, and `--offline` for cached pricing data.

## What Gets Calculated

- **Token usage** - session usage records count as one call each; otherwise cumulative session shutdown snapshots are converted into per-resume intervals, and OTel rows replace them when the shutdowns miss calls; OTel chat spans are preferred within the OTel source, with inference logs and agent-turn logs used as fallbacks.
- **Cache tokens** - cache read and cache creation token attributes are counted when present.
- **Input tokens** - session-state `inputTokens` and OTel `gen_ai.usage.input_tokens` are normalized to uncached input after subtracting cache reads and cache writes.
- **Reasoning tokens** - session-state reasoning tokens are already included in output tokens; OpenTelemetry reasoning is included when total usage metadata shows it is separate.
- **Recorded credits** - in `auto` and `display` modes, usage records and shutdown intervals that carry `totalNanoAiu` are reported at $0.01 per AI credit; in `display` mode, rows without them are $0.
- **Pricing** - in `auto` mode, rows without recorded credits, and in `--mode calculate`, every row, are priced from LiteLLM pricing data using the normalized model name; both `-1m` and `-1m-internal` suffixes are removed.
- **Unattributed credits** - verified session credits left after accounting for shutdowns and receipts are reported as `unknown` rows at $0.01 per AI credit.

## Environment Variables

| Variable                          | Description                                                                                                   |
| --------------------------------- | ------------------------------------------------------------------------------------------------------------- |
| `COPILOT_HOME`                    | Copilot data root; defaults to `~/.copilot`                                                                   |
| `COPILOT_OTEL_FILE_EXPORTER_PATH` | Explicit Copilot OpenTelemetry JSONL file to include                                                          |
| `LOG_LEVEL`                       | `0` hides aggregated warnings, progress, and table titles ([details](/guide/environment-variables#log-level)) |

## Troubleshooting

::: details No Copilot usage data found
Ensure Copilot session-state files exist under `${COPILOT_HOME:-~/.copilot}/session-state/<session-id>/events.jsonl`, or enable OpenTelemetry file export and place exported `.jsonl` files under `${COPILOT_HOME:-~/.copilot}/otel/`.

If you are using `copilot --resume`, session-state events remain available without OpenTelemetry. OTel-only activity from sessions started without file export cannot be recovered by ccusage.
:::

::: details Costs showing as $0.00
If a model is not in LiteLLM's database, the cost will be $0.00. [Open an issue](https://github.com/ccusage/ccusage/issues/new) to request alias support.
:::
