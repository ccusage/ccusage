# Configuration Overview

ccusage provides multiple ways to configure its behavior, allowing you to customize it for your specific needs. Report options come from command-line options and configuration files; environment variables tell ccusage where each source stores its data.

## Configuration Methods

ccusage supports three configuration methods, each with its own use case:

1. **[Command-Line Options](/guide/cli-options)** - Direct control for individual commands
2. **[Configuration Files](/guide/config-files)** - Persistent, shareable settings
3. **[Environment Variables](/guide/environment-variables)** - Data source directories and log level

## Configuration Priority

Report options are applied in this priority order (highest to lowest):

1. **Command-line arguments** (e.g., `--json`, `--offline`)
2. **The configuration file** - the `--config` file when given, otherwise the first of `.ccusage/ccusage.json` and the user config files that exists and holds a JSON object; see [Configuration Files](/guide/config-files#configuration-priority)
3. **Built-in defaults**

No environment variable sets a report option such as the timezone or the cost mode.

### Priority Example

In `.ccusage/ccusage.json`:

```json
{
	"defaults": {
		"mode": "calculate"
	}
}
```

```bash
# The configuration file sets mode to "calculate"
ccusage daily

# A command-line argument takes priority
ccusage daily --mode display
```

## Quick Start

### Basic Setup

1. **Set custom data directories** (only if not using defaults):

```bash
export CODEX_HOME="$HOME/.codex"
export GEMINI_DATA_DIR="$HOME/.gemini/tmp"
export OPENCODE_DATA_DIR="$HOME/.local/share/opencode"
export AMP_DATA_DIR="$HOME/.local/share/amp"
export CLINE_DATA_DIR="$HOME/.cline,/path/to/globalStorage/saoudrizwan.claude-dev"
export PI_AGENT_DIR="$HOME/.pi/agent/sessions"
export KILO_DATA_DIR="$HOME/.local/share/kilo"
export COPILOT_HOME="$HOME/.copilot"
export COPILOT_OTEL_FILE_EXPORTER_PATH="$COPILOT_HOME/otel/copilot-otel.jsonl"
export ZCODE_HOME="$HOME/.zcode"
export DSH_HOME="$HOME/.dsh"
```

Use comma-separated directories when you want reports to combine multiple profiles or archives:

```bash
export CODEX_HOME="$HOME/.codex,$HOME/.codex-work"
export GEMINI_DATA_DIR="$HOME/.gemini/tmp,/backup/gemini/tmp"
export OPENCODE_DATA_DIR="$HOME/.local/share/opencode,/archive/opencode"
export CLINE_DATA_DIR="$HOME/.cline,/archive/cline"
export KILO_DATA_DIR="$HOME/.local/share/kilo,/backup/kilo"
```

2. **Create a configuration file** for your preferences:

```json
{
	"$schema": "https://ccusage.com/config-schema.json",
	"defaults": {
		"timezone": "America/New_York",
		"breakdown": true
	}
}
```

3. **Use command-line options** for one-off changes:

```bash
ccusage daily --since 20260101 --json
```

## Common Configuration Scenarios

### Personal Development

For individual developers working on multiple projects, in `~/.config/claude/ccusage.json`:

```json
{
	"$schema": "https://ccusage.com/config-schema.json",
	"defaults": {
		"breakdown": true,
		"timezone": "local"
	},
	"claude": {
		"commands": {
			"daily": {
				"instances": true
			}
		}
	}
}
```

`instances` groups `ccusage claude daily` by project; the unified `ccusage daily` has no project grouping.

### Multiple Sources

Configure Claude Code, Codex, OpenCode, Amp, Droid, Codebuff, Cline, Hermes Agent, pi-agent, Goose, OpenClaw, Kilo, Kimi, Qwen, GitHub Copilot CLI, Gemini CLI, Antigravity, Grok Build CLI, ZCode, and DeepSeek Harness separately with data source namespaces in `~/.config/claude/ccusage.json`:

```json
{
	"$schema": "https://ccusage.com/config-schema.json",
	"defaults": {
		"timezone": "UTC"
	},
	"claude": {
		"commands": {
			"daily": {
				"instances": true
			}
		}
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
	"cline": {
		"commands": {
			"daily": {
				"timezone": "UTC"
			}
		}
	}
}
```

Source sections apply to focused commands such as `ccusage codex daily` and `ccusage amp session`. They are also used by unified reports such as `ccusage daily`, where each source receives its own merged options before data is loaded.

### Team Collaboration

For teams sharing configuration, commit `.ccusage/ccusage.json` to the repository:

```json
{
	"$schema": "https://ccusage.com/config-schema.json",
	"defaults": {
		"timezone": "UTC",
		"mode": "auto"
	}
}
```

### CI/CD Pipeline

For automated environments:

```bash
# Environment variables
export CODEX_HOME="/ci/codex-data"
export LOG_LEVEL=0  # Hide warnings, progress, and table titles

# Run with specific options
ccusage daily --offline --json > report.json
```

## Configuration by Feature

### Cost Calculation

Control how costs are calculated:

- **Mode**: `auto` (default), `calculate`, or `display`
- **Offline**: Use embedded pricing data
- **Breakdown**: Show per-model costs

```bash
ccusage daily --mode calculate --breakdown --offline
```

### Date and Time

Customize date/time handling:

- **Timezone**: Any valid timezone (e.g., `UTC`, `America/New_York`)
- **Date Range**: Filter with `--since` and `--until`

```bash
ccusage daily --timezone UTC --since 20260101
```

### Output Format

Control output presentation:

- **JSON**: Machine-readable output with `--json`
- **Debug**: Show detailed information with `--debug`

```bash
ccusage daily --json | jq ".daily[] | select(.totalCost > 10)"
```

### Project Analysis

Analyze Claude Code usage by project with `ccusage claude daily`:

- **Instances**: Group by project with `--instances`
- **Project Filter**: Focus on specific project with `--project`
- **Aliases**: Set custom names via configuration file

In `.ccusage/ccusage.json`:

```json
{
	"commands": {
		"daily": {
			"projectAliases": "uuid-123=My App,long-name=Backend"
		}
	}
}
```

```bash
ccusage claude daily --instances --project uuid-123
```

`--project` matches the project directory name; aliases only change the label shown in the table.

## Debugging Configuration

```bash
# Show skipped input and loader details on stderr
ccusage daily --debug

# Check environment variables
env | grep -E "CLAUDE|CODEX|LOG_LEVEL"

# Also log pricing refresh details
LOG_LEVEL=4 ccusage daily
```

Configuration diagnostics name the selected file and JSON path. In a discovered file, invalid optional fields warn while valid settings remain effective; ccusage does not fall through to another file. Invalid dates are errors for reports that use them. Statusline commands warn about invalid discovered date bounds and drop the affected bounds. Invalid named Pi stores, unreadable files and invalid JSON may also produce errors. An explicit `--config` file is strict. Unknown keys warn, and `LOG_LEVEL=0` suppresses warnings. Check the [search order](/guide/config-files#configuration-file-locations) when a different file shadows the one you edited.

## Best Practices

### 1. Layer Your Configuration

Use different configuration methods for different scopes:

- **Environment variables**: Machine-specific settings (paths)
- **User config or project config**: Preferences and team standards (timezone, mode, formatting); ccusage reads only one of them, so a project config replaces the user config rather than extending it
- **CLI arguments**: One-off overrides

### 2. Use Configuration Files for Teams

Share consistent settings across team members:

```bash
# Commit to version control
git add .ccusage/ccusage.json
git commit -m "Add team ccusage configuration"
```

### 3. Document Your Configuration

Use a README to explain configuration choices; the JSON file itself cannot contain comments:

```markdown
# ccusage Configuration

Our team uses:

- UTC timezone for consistency
- JSON output for automated processing
- Calculate mode for accurate cost tracking
```

### 4. Validate Configuration

Use the schema for validation:

```json
{
	"$schema": "https://ccusage.com/config-schema.json"
}
```

### 5. Keep Secrets Secure

Never put sensitive information in configuration files:

- ❌ API keys or tokens
- ❌ Personal identifiers
- ✅ Timezone preferences
- ✅ Output formats

## Migration Guide

### From Manual Commands

Convert repeated commands to configuration. Before:

```bash
ccusage claude daily --breakdown --instances --timezone UTC
```

After, with this configuration file:

```json
{
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

```bash
ccusage claude daily
```

## Troubleshooting

### Common Issues

1. **Configuration not applied**: Check which file ccusage reads; it uses only the first valid one
2. **Invalid JSON**: Validate syntax with `jq`; a file that does not parse is skipped without an error, and comments are not allowed
3. **Directory not found**: Verify the data source environment variable
4. **No data**: Check directory permissions

### Getting Help

If configuration issues persist:

1. Run with debug mode: `ccusage daily --debug`
2. Log pricing refresh details: `LOG_LEVEL=4 ccusage daily`
3. Validate JSON config: `jq . < ccusage.json`
4. Report issues on [GitHub](https://github.com/ccusage/ccusage/issues)

## Next Steps

Explore specific configuration topics:

- [Command-Line Options](/guide/cli-options) - All available CLI arguments
- [Environment Variables](/guide/environment-variables) - System configuration
- [Configuration Files](/guide/config-files) - Persistent settings
- [Claude Code](/guide/claude/) - Claude Code data discovery
- [Cost Modes](/guide/cost-modes) - Understanding calculation modes
- [Claude Code](/guide/claude/) - Claude Code data paths and source-specific options
