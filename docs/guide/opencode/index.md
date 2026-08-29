# OpenCode Data Source (Beta)

> OpenCode support is experimental. Expect breaking changes while both ccusage and [OpenCode](https://github.com/opencode-ai/opencode) continue to evolve.

ccusage can read [OpenCode](https://github.com/opencode-ai/opencode) session logs as one of its supported local data sources. OpenCode is a terminal-based AI coding assistant that supports multiple AI providers.

## Focused Views

::: code-group

```bash [bunx (Recommended)]
bunx ccusage opencode --help
```

```bash [npx]
npx ccusage@latest opencode --help
```

```bash [pnpm]
pnpm dlx ccusage opencode --help
```

```bash [opencode x]
BUN_BE_BUN=1 opencode x ccusage@latest opencode --help
```

:::

::: tip opencode x option
The `opencode x` option requires the native version of OpenCode. If you installed OpenCode via npm, use the `bunx` or `npx` options instead.
:::

## Data Source

The CLI reads OpenCode message and session JSON files from `OPENCODE_DATA_DIR` when it is set. Otherwise, it uses `${XDG_DATA_HOME:-$HOME/.local/share}/opencode`. `OPENCODE_DATA_DIR` can be one directory or a comma-separated list of directories.

```bash
OPENCODE_DATA_DIR="$HOME/.local/share/opencode,/backup/opencode" ccusage opencode daily
```

<!-- eslint-skip -->

```
${XDG_DATA_HOME:-$HOME/.local/share}/opencode/
└── storage/
    ├── message/{sessionID}/msg_{messageID}.json
    └── session/{projectHash}/{sessionID}.json
```

## Report Views

| Focused view               | Description                                          | See also                                |
| -------------------------- | ---------------------------------------------------- | --------------------------------------- |
| `ccusage opencode daily`   | Aggregate usage by date (YYYY-MM-DD)                 | [Daily Usage](/guide/daily-reports)     |
| `ccusage opencode weekly`  | Aggregate usage by ISO week (YYYY-Www)               | [Weekly Usage](/guide/weekly-reports)   |
| `ccusage opencode monthly` | Aggregate usage by month (YYYY-MM)                   | [Monthly Usage](/guide/monthly-reports) |
| `ccusage opencode session` | Per-session breakdown with parent/subagent hierarchy | [Session Usage](/guide/session-reports) |

These views support `--json` for structured output and `--compact` for narrow terminals. See the linked ccusage documentation for detailed flag descriptions.

## Session Hierarchy

OpenCode supports subagent sessions. The session report displays:

- **Bold titles** for parent sessions with subagents
- **Indented rows** (`↳`) for subagent sessions
- **Subtotal rows** combining parent + subagents

## Environment Variables

| Variable            | Description                                                                                |
| ------------------- | ------------------------------------------------------------------------------------------ |
| `OPENCODE_DATA_DIR` | Override the root directory, or comma-separated root directories, containing OpenCode data |
| `XDG_DATA_HOME`     | Base directory for OpenCode data when `OPENCODE_DATA_DIR` is not set                       |
| `LOG_LEVEL`         | Adjust verbosity (0 silent ... 5 trace)                                                    |

## Cost Calculation

OpenCode stores `cost: 0` in message files. Costs are calculated from token counts using LiteLLM pricing. Model aliases (e.g., `gemini-3-pro-high` → `gemini-3-pro-preview`) are handled automatically.

## Troubleshooting

::: details No OpenCode usage data found
Ensure the data directory exists at `${XDG_DATA_HOME:-$HOME/.local/share}/opencode/storage/message/`. Set `OPENCODE_DATA_DIR` for custom paths or comma-separated archive roots.
:::

::: details Costs showing as $0.00
If a model is not in LiteLLM's database, the cost will be $0.00. [Open an issue](https://github.com/ccusage/ccusage/issues/new) to request alias support.
:::
