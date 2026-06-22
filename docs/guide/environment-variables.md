# Environment Variables

ccusage supports several environment variables for configuration and customization. Environment variables provide a way to configure ccusage without modifying command-line arguments or configuration files.

## Agent Data Directories

ccusage detects supported data source files from conventional locations by default. Set these variables when your data lives somewhere else. Directory variables can be one directory or a comma-separated list of directories; the Copilot variable points at one explicit JSONL export file:

| Variable                          | Agent        | Default                            |
| --------------------------------- | ------------ | ---------------------------------- |
| `CLAUDE_CONFIG_DIR`               | Claude Code  | `~/.config/claude` and `~/.claude` |
| `CODEX_HOME`                      | Codex        | `~/.codex`                         |
| `OPENCODE_DATA_DIR`               | OpenCode     | `~/.local/share/opencode`          |
| `AMP_DATA_DIR`                    | Amp          | `~/.local/share/amp`               |
| `DROID_SESSIONS_DIR`              | Droid        | `~/.factory/sessions`              |
| `CODEBUFF_DATA_DIR`               | Codebuff     | `~/.config/manicode`               |
| `HERMES_HOME`                     | Hermes Agent | `~/.hermes`                        |
| `PI_AGENT_DIR`                    | pi-agent     | `~/.pi/agent/sessions`             |
| `GOOSE_PATH_ROOT`                 | Goose        | Standard Goose data roots          |
| `OPENCLAW_DIR`                    | OpenClaw     | `~/.openclaw`                      |
| `KILO_DATA_DIR`                   | Kilo         | `~/.local/share/kilo`              |
| `KIMI_DATA_DIR`                   | Kimi         | `~/.kimi`, `~/.kimi-code`          |
| `QWEN_DATA_DIR`                   | Qwen         | `~/.qwen`                          |
| `COPILOT_OTEL_FILE_EXPORTER_PATH` | Copilot CLI  | Explicit `.jsonl` file             |
| `GEMINI_DATA_DIR`                 | Gemini CLI   | `~/.gemini/tmp`                    |

Example:

```bash
export CODEX_HOME="/path/to/codex,/archive/codex,/path/to/codex-exec-jsonl"
export OPENCODE_DATA_DIR="/path/to/opencode,/archive/opencode"
export AMP_DATA_DIR="/path/to/amp,/archive/amp"
export DROID_SESSIONS_DIR="/path/to/factory/sessions,/archive/factory/sessions"
export CODEBUFF_DATA_DIR="/path/to/manicode,/archive/manicode"
export HERMES_HOME="/path/to/hermes,/archive/hermes"
export PI_AGENT_DIR="/path/to/pi/sessions,/archive/pi/sessions"
export GOOSE_PATH_ROOT="/path/to/goose,/archive/goose"
export OPENCLAW_DIR="/path/to/openclaw,/archive/openclaw"
export KILO_DATA_DIR="/path/to/kilo,/archive/kilo"
export KIMI_DATA_DIR="/path/to/kimi,/archive/kimi"
export QWEN_DATA_DIR="/path/to/qwen,/archive/qwen"
export COPILOT_OTEL_FILE_EXPORTER_PATH="/path/to/copilot-otel.jsonl"
export GEMINI_DATA_DIR="/path/to/gemini/tmp,/archive/gemini/tmp"
ccusage daily
```

Empty entries, directories that do not exist, and missing explicit files are skipped. Duplicate paths are read once.

## CLAUDE_CONFIG_DIR

Specifies where ccusage should look for Claude Code data. See [Claude Code](/guide/claude/) for default paths, multiple-directory behavior, and Claude-specific examples.

## LOG_LEVEL

Controls the verbosity of log output.

### Log Levels

| Level  | Value | Description                  | Use Case               |
| ------ | ----- | ---------------------------- | ---------------------- |
| Silent | `0`   | Errors only                  | Scripts, piping output |
| Warn   | `1`   | Warnings and errors          | CI/CD environments     |
| Log    | `2`   | Normal logs                  | General use            |
| Info   | `3`   | Informational logs (default) | Standard operation     |
| Debug  | `4`   | Debug information            | Troubleshooting        |
| Trace  | `5`   | All operations               | Deep debugging         |

### Usage Examples

```bash
# Silent mode - only show results
LOG_LEVEL=0 ccusage daily

# Warning level - for CI/CD
LOG_LEVEL=1 ccusage monthly

# Debug mode - troubleshooting
LOG_LEVEL=4 ccusage session

# Trace everything - deep debugging
LOG_LEVEL=5 ccusage blocks
```

### Practical Applications

#### Clean Output for Scripts

```bash
# Get clean JSON output without logs
LOG_LEVEL=0 ccusage daily --json | jq '.summary.totalCost'
```

#### CI/CD Pipeline

```bash
# Show only warnings and errors in CI
LOG_LEVEL=1 ccusage daily --instances
```

#### Debugging Issues

```bash
# Maximum verbosity for troubleshooting
LOG_LEVEL=5 ccusage daily --debug
```

#### Piping Output

```bash
# Silent logs when piping to other commands
LOG_LEVEL=0 ccusage monthly --json | python analyze.py
```

## Additional Environment Variables

### CCUSAGE_OFFLINE

Force offline mode by default:

```bash
export CCUSAGE_OFFLINE=1
ccusage daily  # Runs in offline mode
```

### NO_COLOR

Disable colored output (standard CLI convention):

```bash
export NO_COLOR=1
ccusage daily  # No color formatting
```

### FORCE_COLOR

Force colored output even when piping:

```bash
export FORCE_COLOR=1
ccusage daily | less -R  # Preserves colors
```

## Setting Environment Variables

### Temporary (Current Session)

```bash
# Set for single command
LOG_LEVEL=0 ccusage daily

# Set for current shell session
export CODEX_HOME="/path/to/codex,/archive/codex"
ccusage daily
```

### Permanent (Shell Profile)

Add to your shell configuration file:

#### Bash (~/.bashrc)

```bash
export CODEX_HOME="$HOME/.codex"
export LOG_LEVEL=3
```

#### Zsh (~/.zshrc)

```zsh
export CODEX_HOME="$HOME/.codex"
export LOG_LEVEL=3
```

#### Fish (~/.config/fish/config.fish)

```fish
set -x CODEX_HOME "$HOME/.codex"
set -x LOG_LEVEL 3
```

#### PowerShell (Profile.ps1)

```powershell
$env:CODEX_HOME = "$env:USERPROFILE\.codex"
$env:LOG_LEVEL = "3"
```

## Precedence

Environment variables have lower precedence than command-line arguments but higher than configuration files:

1. **Command-line arguments** (highest priority)
2. **Environment variables**
3. **Configuration files**
4. **Built-in defaults** (lowest priority)

Example:

```bash
# Environment variable sets offline mode
export CCUSAGE_OFFLINE=1

# But command-line argument overrides it
ccusage daily --no-offline  # Runs in online mode
```

## Debugging

To see which environment variables are being used:

```bash
# Show all environment variables
env | grep -E "CLAUDE|CODEX|OPENCODE|AMP|DROID|CODEBUFF|HERMES|PI_AGENT|GOOSE|OPENCLAW|KILO|KIMI|QWEN|COPILOT|GEMINI|CCUSAGE|LOG_LEVEL"

# Debug mode shows environment variable usage
LOG_LEVEL=4 ccusage daily --debug
```

## Related Documentation

- [Command-Line Options](/guide/cli-options) - CLI arguments and flags
- [Configuration Files](/guide/config-files) - JSON configuration files
- [Configuration Overview](/guide/configuration) - Complete configuration guide
