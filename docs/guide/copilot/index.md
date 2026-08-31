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

The CLI reads Copilot session-state shutdown events from `${COPILOT_HOME:-~/.copilot}/session-state/*/events.jsonl` by default. It also reads OpenTelemetry JSONL files recursively from `${COPILOT_HOME:-~/.copilot}/otel/**/*.jsonl` and includes the single explicit file pointed to by `COPILOT_OTEL_FILE_EXPORTER_PATH`. Set `COPILOT_HOME` to the Copilot data root when the default `~/.copilot` directory has been relocated.

Session-state files do not require OpenTelemetry configuration. Shutdown usage is cumulative per canonical session/model pair, so only the latest shutdown is retained. For date-bounded reports, ccusage selects the latest snapshot visible through `--until` and subtracts the latest earlier snapshot before `--since` when one exists. When both sources contain the same session/model pair, the session-state usage is used and matching OpenTelemetry rows are suppressed only when their timestamps are at or before the latest canonical shutdown timestamp for that pair; rows emitted after that timestamp by a resumed session are retained. OpenTelemetry rows for other session/model pairs remain available.

Only `session.shutdown` events are used. For each `data.modelMetrics.<model>` entry, ccusage reads its `usage` fields and, when greater than one, `requests.count` for `messageCount`; `requests.cost` is ignored and costs continue to use ccusage's normal token pricing. In session-state data, `inputTokens` includes both cache buckets, so ccusage derives uncached input as `max(inputTokens - cacheReadTokens - cacheWriteTokens, 0)` before populating `inputTokens`; cache reads and cache writes are then reported separately. `reasoningTokens` is a subset of `outputTokens`, so it is not added again to output, total tokens, or cost. Copilot model IDs with `-1m` or `-1m-internal` suffixes, such as `claude-opus-4.6-1m`, are normalized to their priced model name for pricing and source deduplication.

For example, a `claude-opus-4.7` shutdown with `inputTokens=100`, `outputTokens=50`, `cacheReadTokens=10`, and `cacheWriteTokens=20` is reported as 70 input tokens, 50 output tokens, 20 cache-creation tokens, and 10 cache-read tokens. With the embedded Opus pricing, its calculated cost is exactly `$0.00173`.

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
| `ccusage copilot monthly` | Aggregate usage by month           | [Monthly Usage](/guide/monthly-reports) |
| `ccusage copilot session` | Group usage by Copilot session IDs | [Session Usage](/guide/session-reports) |

These views support `--json` for structured output, `--compact` for narrow terminals, and `--offline` for cached pricing data.

## What Gets Calculated

- **Token usage** - the latest cumulative session shutdown usage is read from session-state files; OTel chat spans are preferred within the OTel source, with inference logs and agent-turn logs used as fallbacks.
- **Cache tokens** - cache read and cache creation token attributes are counted when present.
- **Input tokens** - session-state `inputTokens` is normalized to uncached input after subtracting cache reads and cache writes.
- **Reasoning tokens** - session-state reasoning tokens are already included in output tokens; OpenTelemetry reasoning is included when total usage metadata shows it is separate.
- **Pricing** - costs are calculated from LiteLLM pricing data using the normalized model name; both `-1m` and `-1m-internal` suffixes are removed.

## Environment Variables

| Variable                          | Description                                          |
| --------------------------------- | ---------------------------------------------------- |
| `COPILOT_HOME`                    | Copilot data root; defaults to `~/.copilot`          |
| `COPILOT_OTEL_FILE_EXPORTER_PATH` | Explicit Copilot OpenTelemetry JSONL file to include |
| `LOG_LEVEL`                       | Adjust verbosity (0 silent ... 5 trace)              |

## Troubleshooting

::: details No Copilot usage data found
Ensure Copilot session-state files exist under `${COPILOT_HOME:-~/.copilot}/session-state/<session-id>/events.jsonl`, or enable OpenTelemetry file export and place exported `.jsonl` files under `${COPILOT_HOME:-~/.copilot}/otel/`.

If you are using `copilot --resume`, session-state events remain available without OpenTelemetry. OTel-only activity from sessions started without file export cannot be recovered by ccusage.
:::

::: details Costs showing as $0.00
If a model is not in LiteLLM's database, the cost will be $0.00. [Open an issue](https://github.com/ccusage/ccusage/issues/new) to request alias support.
:::
