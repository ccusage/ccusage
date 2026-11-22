# Environment Variables

ccusage reads environment variables for where each data source stores its logs, for log verbosity, and for terminal colors. Report options such as the timezone or offline mode have no environment variable; set them with [command-line options](/guide/cli-options) or a [configuration file](/guide/config-files).

## Agent Data Directories

ccusage detects supported data source files from conventional locations by default. Set these variables when your data lives somewhere else. Directory variables can be one directory or a comma-separated list of directories; `COPILOT_HOME`, `GROK_HOME`, and `GOOSE_PATH_ROOT` accept a single root, while `COPILOT_OTEL_FILE_EXPORTER_PATH` points at one explicit JSONL export file:

| Variable                          | Agent            | Default                                                  |
| --------------------------------- | ---------------- | -------------------------------------------------------- |
| `CLAUDE_CONFIG_DIR`               | Claude Code      | `~/.config/claude` and `~/.claude`                       |
| `CODEX_HOME`                      | Codex            | `~/.codex`                                               |
| `OPENCODE_DATA_DIR`               | OpenCode         | `${XDG_DATA_HOME:-$HOME/.local/share}/opencode`          |
| `AMP_DATA_DIR`                    | Amp              | `~/.local/share/amp`                                     |
| `DROID_SESSIONS_DIR`              | Droid            | `~/.factory/sessions`                                    |
| `CODEBUFF_DATA_DIR`               | Codebuff         | `~/.config/manicode`                                     |
| `CLINE_DATA_DIR`                  | Cline            | `~/.cline` and supported editor `globalStorage` roots    |
| `HERMES_HOME`                     | Hermes Agent     | `~/.hermes`                                              |
| `PI_AGENT_DIR`                    | pi-agent         | `~/.pi/agent/sessions`                                   |
| `GOOSE_PATH_ROOT`                 | Goose            | Standard Goose data roots                                |
| `OPENCLAW_DIR`                    | OpenClaw         | `~/.openclaw`, `~/.clawdbot`, `~/.moltbot`, `~/.moldbot` |
| `KILO_DATA_DIR`                   | Kilo             | `~/.local/share/kilo`                                    |
| `KIMI_DATA_DIR`                   | Kimi             | `~/.kimi`, `~/.kimi-code`                                |
| `QWEN_DATA_DIR`                   | Qwen             | `~/.qwen`                                                |
| `COPILOT_HOME`                    | Copilot CLI      | `~/.copilot`                                             |
| `COPILOT_OTEL_FILE_EXPORTER_PATH` | Copilot CLI      | Explicit `.jsonl` file                                   |
| `GEMINI_DATA_DIR`                 | Gemini CLI       | `~/.gemini/tmp`                                          |
| `ANTIGRAVITY_DATA_DIR`            | Antigravity      | `~/.gemini/antigravity*` and `~/.config/antigravity`     |
| `GROK_HOME`                       | Grok Build CLI   | `~/.grok`                                                |
| `ZCODE_HOME`                      | ZCode            | `~/.zcode`                                               |
| `DSH_HOME`                        | DeepSeek Harness | `~/.dsh`                                                 |

Example:

```bash
export CODEX_HOME="/path/to/codex,/archive/codex,/path/to/codex-exec-jsonl"
export OPENCODE_DATA_DIR="/path/to/opencode,/archive/opencode"
export AMP_DATA_DIR="/path/to/amp,/archive/amp"
export DROID_SESSIONS_DIR="/path/to/factory/sessions,/archive/factory/sessions"
export CODEBUFF_DATA_DIR="/path/to/manicode,/archive/manicode"
export CLINE_DATA_DIR="/path/to/cline,/archive/cline"
export HERMES_HOME="/path/to/hermes,/archive/hermes"
export PI_AGENT_DIR="/path/to/pi/sessions,/archive/pi/sessions"
export GOOSE_PATH_ROOT="/path/to/goose"
export OPENCLAW_DIR="/path/to/openclaw,/archive/openclaw"
export KILO_DATA_DIR="/path/to/kilo,/archive/kilo"
export KIMI_DATA_DIR="/path/to/kimi,/archive/kimi"
export QWEN_DATA_DIR="/path/to/qwen,/archive/qwen"
export COPILOT_HOME="/path/to/copilot"
export COPILOT_OTEL_FILE_EXPORTER_PATH="/path/to/copilot-otel.jsonl"
export GEMINI_DATA_DIR="/path/to/gemini/tmp,/archive/gemini/tmp"
export ANTIGRAVITY_DATA_DIR="/path/to/antigravity,/archive/antigravity"
export GROK_HOME="/path/to/grok-home"
export ZCODE_HOME="/path/to/zcode-home,/archive/zcode-home"
export DSH_HOME="/path/to/dsh-home,/archive/dsh-home"
ccusage daily
```

For example, `GOOSE_PATH_ROOT="/path/to/goose"` reads `/path/to/goose/data/sessions/sessions.db`.

For directory variables, a blank override falls back to the default locations, and a leading `~` expands to the home directory. Empty list entries, directories that do not exist, and missing explicit files are skipped. Canonical duplicates of these directory roots are read once. Without a home directory or a configured location, these sources have no default roots. A non-empty `ZCODE_HOME` containing only invalid roots does not fall back to `~/.zcode`.

## CLAUDE_CONFIG_DIR

Specifies where ccusage should look for Claude Code data. See [Claude Code](/guide/claude/) for default paths, multiple-directory behavior, and Claude-specific examples.

## Telemetry

`CCUSAGE_TELEMETRY_DISABLED`, `DO_NOT_TRACK`, and `CI` disable the anonymous daily usage ping when their value is non-empty and is neither `0` nor `false`. The comparison ignores surrounding whitespace, and `false` is case-insensitive. `CCUSAGE_TELEMETRY_DEBUG=1` prints the payload instead of sending it. See [Telemetry](/guide/telemetry) for exactly what is sent.

## LOG_LEVEL

Controls warnings, progress, and pricing logs. `LOG_LEVEL=0` also hides table titles; table rows and JSON data are unchanged. `--debug` still prints skipped-input details.

| Value          | Effect                                                         |
| -------------- | -------------------------------------------------------------- |
| `0`            | Silence aggregated warnings, progress output, and table titles |
| `4` or higher  | Also log pricing refresh details                               |
| unset or other | Default: warnings and progress output                          |

### Usage Examples

```bash
# Get clean JSON output without logs
LOG_LEVEL=0 ccusage daily --json | jq '.totals.totalCost'

# Silent logs when piping to other commands
LOG_LEVEL=0 ccusage monthly --json | python analyze.py

# See pricing refresh details while troubleshooting
LOG_LEVEL=4 ccusage daily --debug
```

## Additional Environment Variables

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
```

#### Zsh (~/.zshrc)

```zsh
export CODEX_HOME="$HOME/.codex"
```

#### Fish (~/.config/fish/config.fish)

```fish
set -x CODEX_HOME "$HOME/.codex"
```

#### PowerShell (Profile.ps1)

```powershell
$env:CODEX_HOME = "$env:USERPROFILE\.codex"
```

## Precedence

The data directory variables replace a source's default locations; they do not interact with report options. Report options come only from command-line arguments, then the [configuration file](/guide/config-files#configuration-priority), then built-in defaults. Some sources also accept a path option, such as `--pi-path` or `pi.defaults.piPath`, which takes precedence over the source's environment variable.

## Debugging

To see which environment variables are set:

```bash
env | grep -E "CLAUDE|CODEX|OPENCODE|AMP|DROID|CODEBUFF|CLINE|HERMES|PI_AGENT|GOOSE|OPENCLAW|KILO|KIMI|QWEN|COPILOT|GEMINI|ANTIGRAVITY|GROK|ZCODE|DSH|LOG_LEVEL"
```

## Related Documentation

- [Command-Line Options](/guide/cli-options) - CLI arguments and flags
- [Configuration Files](/guide/config-files) - JSON configuration files
- [Configuration Overview](/guide/configuration) - Complete configuration guide
