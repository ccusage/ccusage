# Qwen Data Source (Experimental)

> Qwen support is experimental. Expect breaking changes while both ccusage and [Qwen Code](https://github.com/QwenLM/qwen-code) continue to evolve.

ccusage can read Qwen Code chat JSONL files as one of its supported local data sources, using the same focused and all-source report views as the rest of ccusage.

## Focused Views

::: code-group

```bash [bunx (Recommended)]
bunx ccusage qwen --help
```

```bash [npx]
npx ccusage@latest qwen --help
```

```bash [pnpm]
pnpm dlx ccusage qwen --help
```

:::

## Data Source

The CLI reads Qwen chat JSONL files from `QWEN_DATA_DIR` (defaults to `~/.qwen`). `QWEN_DATA_DIR` can be one directory or a comma-separated list of directories.

```bash
QWEN_DATA_DIR="$HOME/.qwen,/backup/qwen" ccusage qwen daily
```

```text
~/.qwen/
└── projects/
    └── {project}/
        └── chats/
            ├── *.jsonl
            └── archive/
                └── *.jsonl
```

Sessions that Qwen Code archives to `chats/archive/` are read as well, so archived usage stays in the reports. A record found in both `chats/` and `chats/archive/` counts once.

## Report Views

| Focused view           | Description                 | See also                                |
| ---------------------- | --------------------------- | --------------------------------------- |
| `ccusage qwen daily`   | Aggregate usage by date     | [Daily Usage](/guide/daily-reports)     |
| `ccusage qwen monthly` | Aggregate usage by month    | [Monthly Usage](/guide/monthly-reports) |
| `ccusage qwen session` | Group usage by Qwen session | [Session Usage](/guide/session-reports) |

These views support `--json` for structured output, `--compact` for narrow terminals, and `--offline` for cached pricing data.

## What Gets Calculated

- **Token usage** - Qwen assistant rows provide input and output token counts through `usageMetadata`.
- **Reasoning tokens** - `thoughtsTokenCount` is included in `totalTokens` and priced as output tokens when pricing data is available.
- **Cache tokens** - `cachedContentTokenCount` is treated as cache read tokens. Qwen logs do not currently expose cache creation tokens.
- **Pricing** - Costs are calculated from LiteLLM pricing data using the raw model name and Qwen provider-prefixed candidates.

## Environment Variables

| Variable        | Description                                                                            |
| --------------- | -------------------------------------------------------------------------------------- |
| `QWEN_DATA_DIR` | Override the root directory, or comma-separated root directories, containing Qwen data |
| `LOG_LEVEL`     | Adjust verbosity (0 silent ... 5 trace)                                                |

## Troubleshooting

::: details No Qwen usage data found
Ensure the data directory exists at `~/.qwen/projects/{project}/chats/`. Set `QWEN_DATA_DIR` if your Qwen data lives elsewhere or in multiple archive roots.
:::

::: details Costs showing as $0.00
If a model is not in LiteLLM's database, the cost will be $0.00. [Open an issue](https://github.com/ccusage/ccusage/issues/new) to request alias support.
:::
