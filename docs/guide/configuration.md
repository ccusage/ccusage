# Configuration Overview

ccusage provides multiple ways to configure its behavior, allowing you to customize it for your specific needs. Configuration can be done through command-line options, environment variables, configuration files, or a combination of all three.

## Configuration Methods

ccusage supports four configuration methods, each with its own use case:

1. **[Command-Line Options](/guide/cli-options)** - Direct control for individual commands
2. **[Environment Variables](/guide/environment-variables)** - System-wide or session settings
3. **[Configuration Files](/guide/config-files)** - Persistent, shareable settings
4. **Data source directories** - Source-specific defaults and environment variable overrides

## Configuration Priority

Settings are applied in this priority order (highest to lowest):

1. **Command-line arguments** (e.g., `--json`, `--offline`)
2. **Custom config file** (via `--config` flag)
3. **Environment variables** (e.g., `CODEX_HOME`, `LOG_LEVEL`)
4. **Local project config** (`.ccusage/ccusage.json`)
5. **User config** (`~/.config/claude/ccusage.json`)
6. **Legacy user config** (`~/.claude/ccusage.json`)
7. **Built-in defaults**

### Priority Example

```bash
# Configuration file sets mode to "calculate"
# .ccusage/ccusage.json
{
  "defaults": {
    "mode": "calculate"
  }
}

# Environment variable sets timezone
export CCUSAGE_TIMEZONE="Asia/Tokyo"

# Command-line argument takes highest priority
ccusage daily --mode display --timezone UTC
# Result: mode=display (CLI), timezone=UTC (CLI)
```

## Quick Start

### Basic Setup

1. **Set custom data directories** (only if not using defaults):

```bash
export CODEX_HOME="$HOME/.codex"
export GEMINI_DATA_DIR="$HOME/.gemini/tmp"
export OPENCODE_DATA_DIR="$HOME/.local/share/opencode"
export AMP_DATA_DIR="$HOME/.local/share/amp"
export PI_AGENT_DIR="$HOME/.pi/agent/sessions"
export KILO_DATA_DIR="$HOME/.local/share/kilo"
export COPILOT_OTEL_FILE_EXPORTER_PATH="$HOME/.copilot/otel/copilot-otel.jsonl"
```

Use comma-separated directories when you want reports to combine multiple profiles or archives:

```bash
export CODEX_HOME="$HOME/.codex,$HOME/.codex-work"
export GEMINI_DATA_DIR="$HOME/.gemini/tmp,/backup/gemini/tmp"
export OPENCODE_DATA_DIR="$HOME/.local/share/opencode,/archive/opencode"
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

For individual developers working on multiple projects:

```json
// ~/.config/claude/ccusage.json
{
	"$schema": "https://ccusage.com/config-schema.json",
	"defaults": {
		"breakdown": true,
		"timezone": "local"
	},
	"commands": {
		"daily": {
			"instances": true
		}
	}
}
```

### Multiple Sources

Configure Claude Code, Codex, OpenCode, Amp, Droid, Codebuff, Hermes Agent, pi-agent, Goose, OpenClaw, Kilo, Kimi, Qwen, GitHub Copilot CLI, and Gemini CLI separately with data source namespaces:

```json
// ~/.config/claude/ccusage.json
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
			"offline": true,
			"bySource": true
		},
		"commands": {
			"daily": {
				"since": "20260101",
				"until": "20260131"
			}
		}
	}
}
```

Source sections apply to focused commands such as `ccusage codex daily` and `ccusage amp session`. They are also used by unified reports such as `ccusage daily`, where each source receives its own merged options before data is loaded.

### Team Collaboration

For teams sharing configuration:

```json
// .ccusage/ccusage.json (committed to repo)
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
export LOG_LEVEL=1  # Warnings only

# Run with specific options
ccusage daily --offline --json > report.json
```

## Configuration by Feature

### Cost Calculation

Control how costs are calculated:

- **Mode**: `auto` (default), `calculate`, or `display`
- **Offline**: Use cached pricing data
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
ccusage daily --json | jq ".data[] | select(.cost > 10)"
```

### Project Analysis

Analyze usage by project:

- **Instances**: Group by project with `--instances`
- **Project Filter**: Focus on specific project with `--project`
- **Aliases**: Set custom names via configuration file

```json
// .ccusage/ccusage.json
{
	"commands": {
		"daily": {
			"projectAliases": "uuid-123=My App,long-name=Backend"
		}
	}
}
```

```bash
ccusage daily --instances --project "My App"
```

## Debugging Configuration

Use debug mode to understand configuration loading:

```bash
# See which config files are loaded
ccusage daily --debug

# Check environment variables
env | grep -E "CLAUDE|CCUSAGE|LOG_LEVEL"

# Verbose logging
LOG_LEVEL=5 ccusage daily
```

### Debug Output

Debug mode shows:

- Config file discovery and loading
- Option merging from different sources
- Final configuration values
- Pricing calculation details

## Best Practices

### 1. Layer Your Configuration

Use different configuration methods for different scopes:

- **Environment variables**: Machine-specific settings (paths)
- **User config**: Personal preferences (timezone)
- **Project config**: Team standards (mode, formatting)
- **CLI arguments**: One-off overrides

### 2. Use Configuration Files for Teams

Share consistent settings across team members:

```bash
# Commit to version control
git add .ccusage/ccusage.json
git commit -m "Add team ccusage configuration"
```

### 3. Document Your Configuration

Add comments or README files explaining configuration choices:

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

### From v1 to v2

If upgrading from older versions:

1. Update directory paths (now supports `~/.config/claude`)
2. Migrate environment variables to config files
3. Update any scripts using old CLI options

### From Manual Commands

Convert repeated commands to configuration:

```bash
# Before: Repeated commands
ccusage daily --breakdown --instances --timezone UTC

# After: Configuration file
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

# Simplified command
ccusage daily
```

## Troubleshooting

### Common Issues

1. **Configuration not applied**: Check priority order
2. **Invalid JSON**: Validate syntax with `jq`
3. **Directory not found**: Verify the data source environment variable
4. **No data**: Check directory permissions

### Getting Help

If configuration issues persist:

1. Run with debug mode: `ccusage daily --debug`
2. Check verbose logs: `LOG_LEVEL=5 ccusage daily`
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
