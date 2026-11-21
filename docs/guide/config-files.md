# Configuration Files

ccusage supports JSON configuration files for persistent settings. Configuration files allow you to set default options for all commands or customize behavior for specific commands without repeating options every time.

## Quick Start

### 1. Use Schema for IDE Support

Always include the schema for autocomplete and validation:

```json
{
	"$schema": "https://ccusage.com/config-schema.json"
}
```

### 2. Set Common Defaults

Put frequently used options in `defaults`:

```json
{
	"$schema": "https://ccusage.com/config-schema.json",
	"defaults": {
		"timezone": "UTC",
		"breakdown": true
	}
}
```

### 3. Override for Specific Commands

Only daily reports show the breakdown here:

```json
{
	"$schema": "https://ccusage.com/config-schema.json",
	"defaults": {
		"breakdown": false
	},
	"commands": {
		"daily": {
			"breakdown": true
		}
	}
}
```

Configuration files are plain JSON: comments are not allowed, and a file with a syntax error is skipped as if it did not exist.

### 4. Convert CLI Arguments to Config

If you find yourself repeating CLI arguments:

```bash
# Before (repeated CLI arguments)
ccusage claude daily --breakdown --instances --timezone UTC
ccusage monthly --breakdown --timezone UTC
```

Convert them to a `ccusage.json` config file:

```json
{
	"$schema": "https://ccusage.com/config-schema.json",
	"defaults": {
		"breakdown": true,
		"timezone": "UTC"
	},
	"commands": {
		"daily": {
			"instances": true
		}
	}
}
```

Now simpler commands:

```bash
ccusage claude daily
ccusage monthly
```

## Configuration File Locations

ccusage searches for configuration files in these locations, in order:

1. **Local project**: `.ccusage/ccusage.json` in the current directory
2. **User config**: `ccusage.json` in the Claude Code directories, described below

The user config lives in the same Claude Code directories ccusage reads usage logs from: `$XDG_CONFIG_HOME/claude/ccusage.json` (`~/.config/claude/ccusage.json` when `XDG_CONFIG_HOME` is unset or empty), then `~/.claude/ccusage.json`. If `CLAUDE_CONFIG_DIR` is set, ccusage looks for `ccusage.json` in each of its comma-separated directories instead, in the order they are listed. An entry of `~` or starting with `~/` is resolved against your home directory (`~user` and `~\` are not expanded), and an entry pointing at an existing `projects/` directory is read as its parent.

ccusage tries the paths in order. It warns and continues if a discovered file contains malformed JSON or the top-level value is anything other than an object. ccusage loads a single configuration file. A valid local project config takes precedence over your user config.

For a discovered JSON object, recoverable field errors produce a warning naming the file and JSON path. The valid settings in that file still apply. An unreadable file stops discovery. Invalid date bounds, invalid `pi.stores`, and an invalid effective `timezone` are errors for reports that use them. Statusline can recover from errors in discovered configuration and still render. With `--config`, malformed JSON and invalid settings are errors.

Unknown keys warn. Set `LOG_LEVEL=0` to suppress warnings. When discovery finds no usable file, ccusage uses the built-in defaults.

## Basic Configuration

Create a `ccusage.json` file with your preferred defaults:

```json
{
	"$schema": "https://ccusage.com/config-schema.json",
	"defaults": {
		"json": false,
		"mode": "auto",
		"offline": false,
		"noCost": false,
		"timezone": "Asia/Tokyo",
		"breakdown": true
	}
}
```

## Configuration Structure

### Schema Support

Add the `$schema` property to get IntelliSense and validation in your IDE:

```json
{
	"$schema": "https://ccusage.com/config-schema.json"
}
```

You can also reference a local schema file after installing ccusage:

```json
{
	"$schema": "./node_modules/ccusage/config-schema.json"
}
```

### Global Defaults

The `defaults` section sets shared default values for unified and source-focused reports:

```json
{
	"$schema": "https://ccusage.com/config-schema.json",
	"defaults": {
		"since": "20260101",
		"until": "20260531",
		"json": false,
		"mode": "auto",
		"debug": false,
		"debugSamples": 5,
		"order": "asc",
		"breakdown": false,
		"offline": false,
		"noCost": false,
		"timezone": "UTC"
	}
}
```

Set `noCost` to `true` to hide cost columns in tables and remove cost fields from JSON output by default.

### Command-Specific Configuration

Override shared defaults for specific unified and source-focused reports using the `commands` section:

```json
{
	"$schema": "https://ccusage.com/config-schema.json",
	"defaults": {
		"mode": "auto",
		"offline": false
	},
	"commands": {
		"daily": {
			"instances": true,
			"breakdown": true
		},
		"blocks": {
			"active": true,
			"tokenLimit": "500000"
		}
	}
}
```

### Source-Specific Configuration

Use data source namespaces to set defaults and report overrides. Supported namespaces are `claude`, `codex`, `opencode`, `amp`, `droid`, `codebuff`, `cline`, `hermes`, `pi`, `goose`, `openclaw`, `kilo`, `kimi`, `qwen`, `copilot`, `gemini`, `antigravity`, `grok`, `zcode`, and `dsh`.

```json
{
	"$schema": "https://ccusage.com/config-schema.json",
	"defaults": {
		"json": false,
		"timezone": "UTC"
	},
	"codex": {
		"defaults": {
			"json": true,
			"offline": true
		},
		"commands": {
			"daily": {
				"since": "20260101",
				"until": "20260131"
			}
		}
	},
	"opencode": {
		"commands": {
			"weekly": {
				"timezone": "Europe/London"
			}
		}
	},
	"droid": {
		"defaults": {
			"offline": true
		}
	},
	"codebuff": {
		"commands": {
			"daily": {
				"json": true
			}
		}
	},
	"cline": {
		"commands": {
			"daily": {
				"timezone": "UTC"
			}
		}
	},
	"pi": {
		"stores": [
			{
				"name": "omp",
				"path": "~/.omp/agent/sessions"
			}
		],
		"defaults": {
			"piPath": "/path/to/pi/sessions,/archive/pi/sessions",
			"piAgentDir": "~/.pi-work/agent"
		}
	},
	"openclaw": {
		"defaults": {
			"openClawPath": "/path/to/openclaw,/archive/openclaw"
		}
	},
	"kilo": {
		"defaults": {
			"offline": true
		}
	},
	"kimi": {
		"defaults": {
			"offline": true
		}
	},
	"qwen": {
		"defaults": {
			"offline": true
		}
	},
	"copilot": {
		"defaults": {
			"offline": true
		}
	},
	"gemini": {
		"defaults": {
			"offline": true
		}
	},
	"zcode": {
		"defaults": {
			"offline": true
		}
	},
	"dsh": {
		"defaults": {
			"offline": true
		}
	}
}
```

This configuration affects source-focused commands such as:

```bash
ccusage codex daily
ccusage opencode weekly
ccusage droid daily
ccusage codebuff daily
ccusage cline daily
ccusage pi daily
ccusage openclaw daily
ccusage kilo daily
ccusage kimi daily
ccusage qwen daily
ccusage copilot monthly
ccusage gemini daily
ccusage antigravity daily
ccusage zcode daily
ccusage dsh daily
```

Source-specific settings are also applied when running unified reports such as `ccusage daily`. Each source uses the same merged options as its focused command: global `defaults`, `commands.<report>`, `commands."<source> <report>"`, `commands."<source>:<report>"`, `<source>.defaults`, and `<source>.commands.<report>`, with command-line flags taking precedence for every source. Set `offline` and `pricingOverrides` globally to share them across sources, or in a source section to specialize them. Set `claude.commands.daily.project` to filter Claude rows in unified daily reports and their sections. Unified weekly reports use the unified command's `startOfWeek` for every source and for `--last` calculation; source-level `startOfWeek` values are ignored. Each source groups dates with its resolved timezone before rows are combined, so sources with different timezones can place the same event on adjacent calendar dates.

Use `pi.stores` for additional pi-format session stores, such as tools or forks that write sessions outside `~/.pi/agent/sessions`. Named stores are additive to the default `pi` agent in unified reports, use their own agent name in JSON and tables, and prefix models with `[name]` followed by a space. Store names must match `^[a-z][a-z0-9_-]{0,31}$`, must be unique, and cannot use a built-in agent name. Each store path can be one sessions directory or a comma-separated list; `~` is expanded for store paths, nonexistent paths are treated as empty, and resolved paths that overlap the default `pi` store or another named store — including one path nested inside another — are rejected. They do not create focused commands such as `ccusage omp daily`; `PI_AGENT_DIR`, `--pi-path`, `--pi-agent-dir`, `pi.defaults.piPath`, and `pi.defaults.piAgentDir` still affect only the default `pi` agent. A store path names the sessions directory itself; ccusage does not switch to a `sessions` child.

Named stores use the unified report's options and pricing overrides; `pi.defaults` and `pi.commands` apply only to the default `pi` source.

For a source-focused command, or each source within a unified report, options are applied in this order:

1. `defaults`
2. `commands.<report>`
3. `commands."<source> <report>"`
4. `commands."<source>:<report>"`
5. `<source>.defaults`
6. `<source>.commands.<report>`
7. Command-line arguments

`commands."codex daily"` and `commands."codex:daily"` both override `commands.daily`; if both are set, the colon form wins.

## Command-Specific Options

### Daily Command

```json
{
	"commands": {
		"daily": {
			"instances": true,
			"project": "my-project",
			"breakdown": true,
			"since": "20260101",
			"until": "20260531"
		}
	}
}
```

### Weekly Command

```json
{
	"commands": {
		"weekly": {
			"startOfWeek": "monday",
			"breakdown": true,
			"timezone": "Europe/London"
		}
	}
}
```

### Monthly Command

```json
{
	"commands": {
		"monthly": {
			"breakdown": true,
			"mode": "calculate"
		}
	}
}
```

`instances`, `project`, and `projectAliases` apply to `ccusage claude daily`.

### Session Command

```json
{
	"commands": {
		"session": {
			"json": true,
			"order": "desc"
		}
	}
}
```

### Blocks Command

```json
{
	"commands": {
		"blocks": {
			"active": true,
			"recent": false,
			"tokenLimit": "max",
			"sessionLength": 5
		}
	}
}
```

The `tokenLimit` value accepts `"max"` or a positive integer, written as a number or a string.

### Statusline

```json
{
	"commands": {
		"statusline": {
			"offline": true,
			"cache": true,
			"refreshInterval": 2,
			"modelLabelAliases": {
				"arn:aws:bedrock:ap-northeast-1:012345678910:application-inference-profile/abcde12345": "claude-opus-4-6"
			}
		}
	}
}
```

## Custom Configuration Files

Use the `--config` option to specify a custom configuration file:

```bash
# Use a specific configuration file
ccusage daily --config ./my-config.json

# Works with all commands
ccusage blocks --config /path/to/team-config.json
```

## Pricing Overrides

When a catalogue lacks your model or its price differs from your contract, set `pricingOverrides` under `defaults`. The [pricing lookup order](/guide/cost-modes) is shared by the adapters.

```json
{
	"$schema": "https://ccusage.com/config-schema.json",
	"defaults": {
		"pricingOverrides": {
			"[pi] gpt-5.4": {
				"inputCostPerToken": 0.0000025,
				"outputCostPerToken": 0.000015,
				"cacheReadInputTokenCost": 0.00000025
			},
			"my-private-claude": {
				"inputCostPerToken": 0.000003,
				"outputCostPerToken": 0.000015,
				"maxInputTokens": 1000000
			}
		}
	}
}
```

### Raw Model Names

Use the model label shown in the report, including any adapter prefix. The override also applies to separator respellings of that label and to every label that ends in it after a provider or host path, such as `zai/glm-4.7` or `azure/gpt-5.5` for a `glm-4.7` or `gpt-5.5` key, even when that host has its own catalog rate. A key for the full label, such as `azure/gpt-5.5`, wins over the bare key for that label. A bare override also applies when the source records an explicit provider, such as a Droid `providerLock` or a Hermes provider.

| Adapter                                | Prefix    | Example key                    |
| -------------------------------------- | --------- | ------------------------------ |
| Pi                                     | `[pi] `   | `[pi] gpt-5.4`                 |
| Named Pi store                         | `[name] ` | `[omp] gpt-5.4`                |
| Others (Claude, Codex, OpenCode, etc.) | none      | `claude-sonnet-4-5`, `gpt-5.5` |

To find the exact name, run `ccusage <agent> daily --json` and look at the `modelName` field in each row's `modelBreakdowns`.
Pi also accepts a bare model name as an override key; that key applies to the default store and any named Pi store using the same model. Use the prefixed label to price one store separately.
A model alias can inherit unspecified fields from its target. An override that sets a token rate (`inputCostPerToken`, `outputCostPerToken`, or a cache rate) is the price at every context size: the model's published long-context tier is dropped, and requests above its threshold use the long-context rates only when the override sets its own `*Above200kTokens` fields, at the model's published threshold (200K tokens when it has none). Above-threshold rates the override leaves out fall back to its base rates. An override that builds on another override, such as one keyed by a model alias, keeps the `*Above200kTokens` rates that the other override sets. An override that sets only `*Above200kTokens` fields changes those rates and keeps the rest of the published tier; one that sets no rates, such as `maxInputTokens` alone, keeps the whole tier. A models.dev base supplies its flat rates and context limit. If no catalog row matches, ccusage uses the model family's cache defaults for any omitted cache-write rates. For an OpenAI-family model such as `gpt-5.5-private`, cache writes default to the input rate.

### Supported Fields

All fields are optional. Unspecified fields keep existing values. Without a pricing entry, token rates default to `0.0`, and speed multipliers use the published Fast, Flex, or Ultrafast rate for that model name or its alias target, or `1.0` if none is published.

- `inputCostPerToken`, `outputCostPerToken` — base per-token rates
- `cacheCreationInputTokenCost`, `cacheReadInputTokenCost` — cache pricing
- `cacheCreationInputTokenCostAbove1hr` — one-hour cache write pricing, including an explicit zero rate
- `inputCostPerTokenAbove200kTokens`, `outputCostPerTokenAbove200kTokens`, `cacheCreationInputTokenCostAbove200kTokens`, `cacheReadInputTokenCostAbove200kTokens` — tiered pricing past 200k tokens
- `maxInputTokens` — context window limit (used by the Claude statusline hook)
- `fastMultiplier` applies when Codex usage uses Fast pricing.
- `flexMultiplier` applies when Codex usage uses Flex pricing.

The Ultrafast multiplier comes only from published rates and cannot be set in `pricingOverrides`; there is no `ultrafastMultiplier` field.

### Overrides vs Offline Mode

`--offline` and `pricingOverrides` are independent:

- `--offline` controls the **data source** — skip the network refresh and use embedded LiteLLM and models.dev pricing.
- `pricingOverrides` controls **specific entries** — patch in or replace prices for individual models.

Overrides apply in both online and offline modes.

For Pi and named Pi stores, Auto mode uses an explicit override to calculate cost from tokens even when the session contains a recorded cost. Display mode continues to show the recorded cost.

This is useful for:

- **Team configurations** - Share configuration files across team members
- **Environment-specific settings** - Different configs for development/production
- **Project-specific overrides** - Use different settings for different projects

## Configuration Example

For a complete configuration example, see [`ccusage.example.json`](https://github.com/ccusage/ccusage/blob/main/ccusage.example.json) in the repository root, which demonstrates:

- Global defaults configuration
- Command-specific overrides
- Examples of supported options and types

## Configuration Priority

Settings are applied in this priority order (highest to lowest):

1. **Command-line arguments** (e.g., `--json`, `--offline`)
2. **The one configuration file ccusage reads**: the `--config` file, or else the first existing file among `.ccusage/ccusage.json`, `~/.config/claude/ccusage.json`, and `~/.claude/ccusage.json` (see [Configuration File Locations](#configuration-file-locations))
3. **Built-in defaults**

Example, in `.ccusage/ccusage.json`:

```json
{
	"defaults": {
		"mode": "calculate"
	}
}
```

```bash
# Config file sets mode to "calculate"
ccusage daily  # Uses mode: calculate

# But CLI argument overrides it
ccusage daily --mode display  # Uses mode: display
```

## Best Practices

### Version Control

For project configs, commit `.ccusage/ccusage.json` to version control:

```bash
# Add to git
git add .ccusage/ccusage.json
git commit -m "Add ccusage configuration"
```

### Document Team Configs

Add comments using a README alongside team configs:

```
team-configs/
├── ccusage.json
└── README.md  # Explain configuration choices
```

## Troubleshooting

### Config Not Being Applied

Check the diagnostic and the selected file:

1. Check that no earlier file in the [search order](#configuration-file-locations) shadows it.
2. Validate JSON syntax; unreadable files and invalid JSON stop loading.
3. Check exact option names; unknown keys produce warnings.
4. In discovered files, correct each invalid field named in a warning. Other valid settings remain effective. Invalid date bounds and named Pi stores stop the command.

Warnings follow `LOG_LEVEL`; use a value above `0` to see them.

### Invalid JSON

Use a JSON validator or IDE with JSON support:

```bash
# Validate JSON syntax
jq . < ccusage.json
```

### Schema Validation Errors

Add `$schema` and let your editor check option names and value types against it:

```json
{
	"$schema": "https://ccusage.com/config-schema.json",
	"commands": {
		"blocks": {
			"tokenLimit": "500000",
			"active": true
		},
		"statusline": {
			"refreshInterval": 2
		}
	}
}
```

## Related Documentation

- [Command-Line Options](/guide/cli-options) - Available CLI arguments
- [Environment Variables](/guide/environment-variables) - Environment configuration
- [Configuration Overview](/guide/configuration) - Complete configuration guide
