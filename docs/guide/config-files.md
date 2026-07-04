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

```json
{
	"$schema": "https://ccusage.com/config-schema.json",
	"defaults": {
		"breakdown": false
	},
	"commands": {
		"daily": {
			"breakdown": true // Only daily needs breakdown
		}
	}
}
```

### 4. Convert CLI Arguments to Config

If you find yourself repeating CLI arguments:

```bash
# Before (repeated CLI arguments)
ccusage daily --breakdown --instances --timezone UTC
ccusage monthly --breakdown --timezone UTC
```

Convert them to a config file:

```json
// ccusage.json
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
ccusage daily
ccusage monthly
```

## Configuration File Locations

ccusage searches for configuration files in these locations (in priority order):

1. **Local project**: `.ccusage/ccusage.json` (higher priority)
2. **User config**: `~/.claude/ccusage.json` or `~/.config/claude/ccusage.json` (lower priority)

Configuration files are merged in priority order, with local project settings overriding user settings.
If you pass a custom config file using `--config`, it will override both local and user configs.
Note that configuration files are not required; if none are found, ccusage will use built-in defaults.
Also, if you have multiple config files, only the first one found will be used.

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

The `defaults` section sets shared default values for unified reports and legacy Claude commands:

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

Override shared defaults for specific unified reports and legacy Claude commands using the `commands` section:

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

Use data source namespaces to set defaults and report overrides. Supported namespaces are `claude`, `codex`, `opencode`, `amp`, `droid`, `codebuff`, `hermes`, `pi`, `goose`, `openclaw`, `kilo`, `kimi`, `qwen`, `copilot`, and `gemini`.

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
	"pi": {
		"stores": [
			{
				"name": "omp",
				"path": "~/.omp/agent/sessions"
			}
		],
		"defaults": {
			"piPath": "/path/to/pi/sessions,/archive/pi/sessions"
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
	}
}
```

This configuration affects source-focused commands such as:

```bash
ccusage codex daily
ccusage opencode weekly
ccusage droid daily
ccusage codebuff daily
ccusage pi daily
ccusage openclaw daily
ccusage kilo daily
ccusage kimi daily
ccusage qwen daily
ccusage copilot monthly
ccusage gemini daily
```

Source-specific settings are also applied when running unified reports such as `ccusage daily`. In that case, each source receives its own merged options before data is loaded.

Use `pi.stores` for additional pi-format session stores, such as tools or forks that write sessions outside `~/.pi/agent/sessions`. Named stores are additive to the default `pi` agent in unified reports, use their own agent name in JSON and tables, and prefix models with `[name]` followed by a space. Store names must match `^[a-z][a-z0-9_-]{0,31}$`, must be unique, and cannot use a built-in agent name. Each store path can be one sessions directory or a comma-separated list; `~` is expanded for store paths, nonexistent paths are treated as empty, and resolved paths that overlap the default `pi` store or another named store — including one path nested inside another — are rejected. They do not create focused commands such as `ccusage omp daily`; `PI_AGENT_DIR`, `--pi-path`, and `pi.defaults.piPath` still affect only the default `pi` agent.

For a namespaced command, options are applied in this order:

1. `defaults`
2. `commands.<report>`
3. `<source>.defaults`
4. `<source>.commands.<report>`
5. Command-line arguments

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

### Session Command

```json
{
	"commands": {
		"session": {
			"id": "abc123-session",
			"project": "my-project",
			"json": true
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
			"sessionLength": 5,
			"live": false,
			"refreshInterval": 1
		}
	}
}
```

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

ccusage looks up token costs from a LiteLLM pricing snapshot embedded in the binary, optionally refreshed at runtime (or skipped with `--offline`). When a model is missing from LiteLLM (private deployments, internal wrappers like Pi's `[pi] gpt-5.4`, custom proxies), or when the snapshot price differs from your contract, set `pricingOverrides` under `defaults` to supply per-model values.

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

Keys in `pricingOverrides` must match the **raw model name** as recorded in the source logs, including any adapter prefix:

| Adapter                                | Prefix    | Example key                    |
| -------------------------------------- | --------- | ------------------------------ |
| Pi                                     | `[pi] `   | `[pi] gpt-5.4`                 |
| Named Pi store                         | `[name] ` | `[omp] gpt-5.4`                |
| Others (Claude, Codex, OpenCode, etc.) | none      | `claude-sonnet-4-5`, `gpt-5.5` |

To find the exact name, run `ccusage <agent> daily --json` and look at the `model` field in the per-row breakdown.

### Supported Fields

All fields are optional. Unspecified fields fall back to the LiteLLM entry (when one exists) or `0.0`:

- `inputCostPerToken`, `outputCostPerToken` — base per-token rates
- `cacheCreationInputTokenCost`, `cacheReadInputTokenCost` — cache pricing
- `inputCostPerTokenAbove200kTokens`, `outputCostPerTokenAbove200kTokens`, `cacheCreationInputTokenCostAbove200kTokens`, `cacheReadInputTokenCostAbove200kTokens` — tiered pricing past 200k tokens
- `maxInputTokens` — context window limit (used by the Claude statusline hook)
- `fastMultiplier` — multiplier applied when the message is recorded as fast-mode

### Overrides vs Offline Mode

`--offline` and `pricingOverrides` are independent:

- `--offline` controls the **data source** — skip the network refresh and use the embedded LiteLLM snapshot only.
- `pricingOverrides` controls **specific entries** — patch in or replace prices for individual models.

Overrides apply in both online and offline modes.

This is useful for:

- **Team configurations** - Share configuration files across team members
- **Environment-specific settings** - Different configs for development/production
- **Project-specific overrides** - Use different settings for different projects

## Configuration Example

For a complete configuration example, see [`/ccusage.example.json`](/ccusage.example.json) in the repository root, which demonstrates:

- Global defaults configuration
- Command-specific overrides
- All available options with proper types

## Configuration Priority

Settings are applied in this priority order (highest to lowest):

1. **Command-line arguments** (e.g., `--json`, `--offline`)
2. **Custom config file** (specified with `--config /path/to/config.json`)
3. **Local project config** (`.ccusage/ccusage.json`)
4. **User config** (`~/.config/claude/ccusage.json`)
5. **Legacy config** (`~/.claude/ccusage.json`)
6. **Built-in defaults**

Example:

```json
// .ccusage/ccusage.json
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

## Debugging Configuration

Use the `--debug` flag to see configuration loading details:

```bash
# Debug configuration loading
ccusage daily --debug

# Debug custom config file
ccusage daily --debug --config ./my-config.json
```

Debug output shows:

- Which config files are checked and found
- Schema and option details from loaded configs
- How options are merged from different sources
- Final values used for each option

Example debug output:

```
[ccusage] ℹ Debug mode enabled - showing config loading details

[ccusage] ℹ Searching for config files:
  • Checking: .ccusage/ccusage.json (found ✓)
  • Checking: ~/.config/claude/ccusage.json (found ✓)
  • Checking: ~/.claude/ccusage.json (not found)

[ccusage] ℹ Loaded config from: .ccusage/ccusage.json
  • Schema: https://ccusage.com/config-schema.json
  • Has defaults: yes (3 options)
  • Has command configs: yes (daily)

[ccusage] ℹ Merging options for 'daily' command:
  • From defaults: mode="auto", offline=false
  • From command config: instances=true
  • From CLI args: debug=true
  • Final merged options: {
      mode: "auto" (from defaults),
      offline: false (from defaults),
      instances: true (from command config),
      debug: true (from CLI)
    }
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

1. Check file location is correct
2. Verify JSON syntax is valid
3. Use `--debug` to see loading details
4. Ensure option names match exactly

### Invalid JSON

Use a JSON validator or IDE with JSON support:

```bash
# Validate JSON syntax
jq . < ccusage.json
```

### Schema Validation Errors

Ensure option values match expected types:

```json
{
	"defaults": {
		"tokenLimit": "500000", // ✅ String or number
		"active": true, // ✅ Boolean
		"refreshInterval": 2 // ✅ Number
	}
}
```

## Related Documentation

- [Command-Line Options](/guide/cli-options) - Available CLI arguments
- [Environment Variables](/guide/environment-variables) - Environment configuration
- [Configuration Overview](/guide/configuration) - Complete configuration guide
