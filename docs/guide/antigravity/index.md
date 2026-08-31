# Antigravity Data Source

ccusage reads Antigravity's local SQLite conversation databases as a standalone
source. It uses the `antigravity` namespace, so Antigravity rows remain
separate from Gemini CLI rows in focused and unified reports.

## Focused Views

```bash
# Daily Antigravity usage
ccusage antigravity daily

# Monthly Antigravity usage
ccusage antigravity monthly

# Antigravity sessions
ccusage antigravity session
```

These views support the shared report options, including `--json`,
`--breakdown`, `--mode`, `--offline`, `--since`, and `--until`.

## Data Source

By default, ccusage scans these local conversation roots:

- `~/.gemini/antigravity/conversations/`
- `~/.gemini/antigravity-cli/conversations/`
- `~/.gemini/antigravity-ide/conversations/`
- `~/.gemini/antigravity-backup/conversations/`
- `~/.config/antigravity/conversations/`

Each root is independent from Gemini CLI discovery. To use one or more custom
roots, set `ANTIGRAVITY_DATA_DIR` to comma-separated directories. A configured
directory can be either an Antigravity data root containing `conversations/` or
the conversation directory itself:

```bash
ANTIGRAVITY_DATA_DIR="$HOME/.gemini/antigravity-cli,/backup/antigravity" \
  ccusage antigravity daily
```

Only `.db` files are read. ccusage opens them read-only and reads the
`gen_metadata` rows in ascending `idx` order.

## What Gets Calculated

- **Input tokens** - The system-prompt and fresh-input buckets are combined.
- **Cache reads** - The cache-read bucket remains separate from uncached input.
- **Output and reasoning** - Visible output and thinking tokens are reported in
  their corresponding output and reasoning fields.
- **Model names** - Antigravity display names and known internal aliases are
  normalized to pricing model IDs. For example, `Gemini 3 Pro` is reported as
  `gemini-3-pro`.
- **Costs** - `calculate` and `auto` use the embedded pricing catalog when a
  matching model is available. Use `--offline` to avoid a pricing refresh.

Continuation rows inherit the most recently observed model in their database.
Rows with a response ID are deduplicated when the same conversation is found in
multiple configured roots.

## Configuration

Use the `antigravity` namespace for source-specific defaults and command
overrides in `ccusage.json`:

```json
{
	"antigravity": {
		"defaults": {
			"offline": true
		},
		"commands": {
			"daily": {
				"timezone": "UTC"
			}
		}
	}
}
```

## Troubleshooting

::: details No Antigravity usage data found
Check that the conversation databases exist below one of the default roots, or
set `ANTIGRAVITY_DATA_DIR` to the directory containing `conversations/`. The
databases must contain the `gen_metadata` table.
:::

::: details A database error is reported
ccusage does not turn unreadable or incomplete SQLite databases into an empty
report. Check the path in the error and ensure the database is readable and has
the expected `gen_metadata (idx, data)` schema.
:::

::: details Costs show as zero
Use `ccusage antigravity daily --mode calculate` and check the model name in
`--json` output. A model that is not in the embedded pricing catalog is reported
without an estimate; you can provide a pricing override through the normal
`pricingOverrides` configuration.
:::
