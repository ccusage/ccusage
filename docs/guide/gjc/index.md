# GJC Data Source

ccusage reads local GJC session transcripts and exposes the same focused and unified reports as the other supported sources.

## Focused Views

```bash
ccusage gjc daily
ccusage gjc monthly
ccusage gjc session
```

Use `ccusage daily`, `ccusage monthly`, or `ccusage session` to include GJC with every other detected source.

## Data Source

GJC stores JSONL transcripts below its config root:

```text
~/${GJC_CONFIG_DIR:-.gjc}/agent/sessions/**/*.jsonl
```

ccusage walks the sessions directory recursively, so top-level sessions and subagent transcripts are included. `GJC_CONFIG_DIR` is a directory name below the home directory, matching GJC itself. Set `GJC_CODING_AGENT_DIR` to an explicit agent directory. When GJC has migrated data to an existing `$XDG_DATA_HOME/gjc` directory, ccusage reads sessions from there.

## Token and Cost Handling

Assistant records with `message.usage` are mapped as follows:

| GJC field    | ccusage field          |
| ------------ | ---------------------- |
| `input`      | Input tokens           |
| `output`     | Output tokens          |
| `cacheRead`  | Cache-read tokens      |
| `cacheWrite` | Cache-creation tokens  |
| `cost.total` | Precomputed USD cost   |

`--mode auto` and `--mode display` use GJC's recorded `cost.total`. `--mode calculate` recalculates cost from the token fields and the shared pricing database. Records without assistant token usage are ignored.

## Configuration

The `gjc` namespace supports the shared report options:

```json
{
	"gjc": {
		"defaults": {
			"offline": true
		},
		"commands": {
			"daily": {
				"json": true
			}
		}
	}
}
```

See [Configuration Files](/guide/config-files), [Environment Variables](/guide/environment-variables), and [JSON Output](/guide/json-output) for shared behavior.

## Troubleshooting

::: details No GJC usage data found
Ensure `~/${GJC_CONFIG_DIR:-.gjc}/agent/sessions` or `$GJC_CODING_AGENT_DIR/sessions` exists and contains GJC JSONL session transcripts. Use `--debug` to report unreadable files.
:::
