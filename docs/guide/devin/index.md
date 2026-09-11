# Devin Data Source (Experimental)

> Devin support is experimental. Expect breaking changes while both ccusage and Devin CLI continue to evolve.

ccusage can read local Devin CLI session transcripts as one of its supported data sources, using the same daily, monthly, and session report views as the rest of ccusage.

## Focused Views

::: code-group

```bash [bunx (Recommended)]
bunx ccusage devin --help
```

```bash [npx]
npx ccusage@latest devin --help
```

```bash [pnpm]
pnpm dlx ccusage devin --help
```

:::

## Data Source

The CLI reads Devin transcript JSON files from `DEVIN_TRANSCRIPTS_DIR` (defaults to `${XDG_DATA_HOME:-$HOME/.local/share}/devin/cli/transcripts`, or `%APPDATA%\devin\cli\transcripts` on Windows). `DEVIN_TRANSCRIPTS_DIR` can be one directory or a comma-separated list of directories.

```bash
DEVIN_TRANSCRIPTS_DIR="$HOME/.local/share/devin/cli/transcripts,/archive/devin/transcripts" ccusage devin session
```

```text
~/.local/share/devin/cli/transcripts/
└── *.json
```

Installs created before the `cognition` → `devin` directory migration are also detected through `~/.local/share/cognition/cli/transcripts`, which the Devin installer links to the new location.

## Report Views

| Focused view            | Description                     | See also                                |
| ----------------------- | ------------------------------- | --------------------------------------- |
| `ccusage devin daily`   | Aggregate usage by date         | [Daily Usage](/guide/daily-reports)     |
| `ccusage devin monthly` | Aggregate usage by month        | [Monthly Usage](/guide/monthly-reports) |
| `ccusage devin session` | Group usage by Devin session    | [Session Usage](/guide/session-reports) |

These views support `--json` for structured output (see [JSON Output](/guide/json-output)), `--compact` for narrow terminals, and `--offline` for cached pricing data.

## What Gets Calculated

- **Token usage** - Each agent step in a transcript reports prompt, completion, cached, and cache-creation tokens. The prompt count includes the cached portions, so ccusage reports the uncached remainder as input tokens, matching how other sources normalize their records.
- **Sessions** - Steps are grouped by `session_id`, with the transcript file name as a fallback when the field is missing. A session that spans midnight attributes each step to the day it ran.
- **Pricing** - Costs are calculated from LiteLLM pricing data for the recorded model, including `cognition/` provider entries when present. Models with no pricing entry, such as `swe-2-*` ids that are not yet in the pricing snapshot, report a cost of zero.
- **Credit usage** - Devin bills in ACUs/credits, which are only exposed through the Devin CLI's own `/usage` command and are not stored in transcripts. ccusage does not estimate credit consumption.

Any transcript version works: agent steps with token metrics, a timestamp, and nonzero usage produce rows — `ATIF-v1.7` stores metrics on the step, `ATIF-v1.4` under `metadata.metrics`. Transcripts from older CLI versions that record no metrics contribute no usage rows.

## Environment Variables

| Variable                | Description                                                                                          |
| ----------------------- | ---------------------------------------------------------------------------------------------------- |
| `DEVIN_TRANSCRIPTS_DIR` | Override the transcripts directory, or comma-separated transcripts directories, containing Devin data |
| `XDG_DATA_HOME`         | Base data directory when `DEVIN_TRANSCRIPTS_DIR` is not set                                          |
| `LOG_LEVEL`             | Adjust verbosity (0 silent ... 5 trace)                                                              |

## Troubleshooting

::: details No Devin usage data found
Ensure the transcripts directory exists at `~/.local/share/devin/cli/transcripts/` (`%APPDATA%\devin\cli\transcripts` on Windows) and contains `*.json` files. Set `DEVIN_TRANSCRIPTS_DIR` if you keep transcripts or `--export`ed files elsewhere. Transcripts from older CLI versions that record no token metrics produce no rows.
:::

::: details Costs showing as $0.00
Transcripts record tokens, not costs. If a model is not in LiteLLM's database — for example `swe-2-*` ids that are not listed yet — the cost will be $0.00 and a warning names the missing model. [Open an issue](https://github.com/ccusage/ccusage/issues/new) to request alias support.
:::
