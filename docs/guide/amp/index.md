# Amp Data Source (Beta)

> Amp support is experimental. Expect breaking changes while both ccusage and [Amp](https://ampcode.com/) continue to evolve.

ccusage reads both current server-backed Amp threads and legacy local thread files, using the same reporting experience as the rest of ccusage: responsive tables, JSON output, LiteLLM-based pricing, cache token accounting, and credit totals where Amp records them.

## Focused Views

::: code-group

```bash [bunx (Recommended)]
bunx ccusage amp --help
```

```bash [npx]
npx ccusage@latest amp --help
```

```bash [pnpm]
pnpm dlx ccusage amp --help
```

:::

## Data Source

By default, ccusage uses the installed, authenticated `amp` CLI to list and export current server-backed threads. It also reads legacy thread JSON files from `~/.local/share/amp/threads/`. Older matching server snapshots are skipped; threads continued after local history stopped are merged without duplicating legacy usage or losing historical credit data.

Set `AMP_DATA_DIR` to read only local archives instead. It can be one directory or a comma-separated list of directories. Explicitly setting it disables server discovery.

```bash
AMP_DATA_DIR="$HOME/.local/share/amp,/backup/amp" ccusage amp session
```

```text
~/.local/share/amp/
└── threads/
    └── **/*.json
```

## Report Views

| Focused view          | Description               | See also                                |
| --------------------- | ------------------------- | --------------------------------------- |
| `ccusage amp daily`   | Aggregate usage by date   | [Daily Usage](/guide/daily-reports)     |
| `ccusage amp monthly` | Aggregate usage by month  | [Monthly Usage](/guide/monthly-reports) |
| `ccusage amp session` | Group usage by Amp thread | [Session Usage](/guide/session-reports) |

These views support `--json` for structured output, `--compact` for narrow terminals, and `--offline` for embedded pricing data. `--since` also avoids exporting server threads that Amp reports as last updated before the requested range.

## What Gets Calculated

- **Token usage** - Amp usage ledger events provide input and output token counts.
- **Cache tokens** - Assistant message usage fields provide cache creation and cache read tokens when available.
- **Credits** - Credit values from legacy Amp ledgers are summed alongside token and cost totals. Current server exports do not expose credits, so newer rows can show zero credits while still including tokens and estimated cost.
- **Pricing** - Costs are calculated from LiteLLM pricing data for Claude and Anthropic model names, including provider-prefixed variants.

## Environment Variables

| Variable       | Description                                                                           |
| -------------- | ------------------------------------------------------------------------------------- |
| `AMP_DATA_DIR` | Use only the given local Amp archive root, or comma-separated roots, instead of server discovery |
| `LOG_LEVEL`    | Adjust verbosity (0 silent ... 5 trace)                                               |

## Troubleshooting

::: details No Amp usage data found
Ensure `amp threads list --json` works and that Amp is authenticated. For legacy archives, ensure the data directory exists at `~/.local/share/amp/threads/`, or set `AMP_DATA_DIR` if the files live elsewhere.
:::

::: details Server-backed totals are incomplete
ccusage prints a warning when the Amp CLI cannot list server threads or when one or more exports fail. Check that `amp` is installed, authenticated, and able to reach the Amp service. Results still include any readable legacy local files.
:::

::: details Costs showing as $0.00
If a model is not in LiteLLM's database, the cost will be $0.00. [Open an issue](https://github.com/ccusage/ccusage/issues/new) to request alias support.
:::
