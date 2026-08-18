# Command-Line Options

ccusage provides extensive command-line options to customize its behavior. These options take precedence over configuration files and environment variables.

## Global Options

All ccusage commands support these global options:

### Date Filtering

Filter usage data by date range:

```bash
# Filter by date range
ccusage daily --since 20260101 --until 20260531

# Show data from a specific date
ccusage monthly --since 20260101

# Show data up to a specific date
ccusage session --until 20260531
```

Both bounds accept `YYYY-MM-DD` or `YYYYMMDD` and are inclusive. Any other spelling, or a value that is not a real calendar date such as `2026-02-30`, is rejected with a non-zero exit code instead of silently changing which rows the report keeps. The same check applies to `since` and `until` in a [configuration file](/guide/config-files).

### Recent Periods

Instead of working out dates, ask for the most recent periods of whatever the report groups by:

```bash
# Today
ccusage daily --last 1

# This week
ccusage weekly --last 1

# This month
ccusage monthly --last 1

# The last seven days, and the last three months
ccusage daily --last 7
ccusage monthly --last 3
```

The count is inclusive of the current period, so `--last 2` on a daily report covers yesterday and today. Weeks start on the same day the report buckets by, which is Monday everywhere except `ccusage claude weekly`, where `--start-of-week` decides.

`--last` works on every daily, weekly, and monthly report, including the per-agent ones such as `ccusage codex daily --last 1`. It is not available on `session`, `blocks`, or `statusline`, which have no calendar period, and it cannot be combined with `--since`, `--until`, or `--sections`.

### Output Format

Control how data is displayed:

```bash
# JSON output for programmatic use
ccusage daily --json
ccusage daily -j

# Show per-model breakdown
ccusage daily --breakdown
ccusage daily -b

# Hide cost columns and JSON cost fields
ccusage daily --no-cost
ccusage daily --json --no-cost

# Combine options
ccusage daily --json --breakdown
```

`--no-cost` removes cost columns from table output and removes cost fields such as `totalCost`, `costUSD`, and `cost` from JSON output.

### Cost Calculation Mode

Choose how costs are calculated:

```bash
# Auto mode (default) - use costUSD when available
ccusage daily --mode auto

# Calculate mode - always calculate from tokens
ccusage daily --mode calculate

# Display mode - only show pre-calculated costUSD
ccusage daily --mode display
```

### Sort Order

Control the ordering of results:

```bash
# Newest first (default)
ccusage daily --order desc

# Oldest first
ccusage daily --order asc
```

### Offline Mode

Run without network connectivity:

```bash
# Use cached pricing data
ccusage daily --offline
ccusage daily -O
```

### Timezone

Set the timezone for date calculations:

```bash
# Use UTC timezone
ccusage daily --timezone UTC

# Use specific timezone
ccusage daily --timezone America/New_York
ccusage daily -z Asia/Tokyo

# Short alias
ccusage monthly -z Europe/London
```

#### Timezone Effect

The timezone affects how usage is grouped by date. For example, usage at 11 PM UTC on January 1st would appear on:

- **January 1st** when `--timezone UTC`
- **January 1st** when `--timezone America/New_York` (6 PM EST)
- **January 2nd** when `--timezone Asia/Tokyo` (8 AM JST next day)

### Debug Options

Get detailed debugging information:

```bash
# Debug mode - show pricing mismatches and config loading
ccusage daily --debug

# Show sample discrepancies
ccusage daily --debug --debug-samples 10
```

### Configuration File

Use a custom configuration file:

```bash
# Specify custom config file
ccusage daily --config ./my-config.json
ccusage monthly --config /path/to/team-config.json
```

## Command-Specific Options

### Unified Report Options

These options apply to `ccusage daily`, `ccusage weekly`, `ccusage monthly`, and `ccusage session` when they are aggregating all detected sources:

```bash
# Emit several JSON report sections from one source load
ccusage daily --sections daily,monthly,session --json

# Add per-agent breakdowns to daily, weekly, and monthly JSON rows
ccusage daily --by-agent --json

# Split Pi-format agents by the provider recorded on each message
ccusage daily --by-provider

# Select agents, scoped providers, and model globs
ccusage daily --agent 'codex,pi[openai-codex]' --model 'gpt-*'
```

`--sections` accepts a comma-separated list of `daily`, `weekly`, `monthly`, and `session`. The invoked report section is always included. For table output, each requested section is printed as a separate table. `--by-agent` is JSON-only; session rows are already per-agent. `--by-provider` splits Pi-format rows using the provider stored on each assistant message. `--agent` accepts comma-separated `agent` or `agent[provider]` selectors, while `--model` accepts comma-separated `*` and `?` glob patterns.

### Daily Command

Additional options for daily reports:

```bash
# Group by project
ccusage daily --instances
ccusage daily -i

# Filter to specific project
ccusage daily --project myproject
ccusage daily -p myproject

# Combine project filtering
ccusage daily --instances --project myproject
```

### Weekly Command

Options for weekly reports:

```bash
# Set week start day
ccusage weekly --start-of-week monday
ccusage weekly --start-of-week sunday
```

### Session Command

Options for session reports:

```bash
# Filter by session ID
ccusage session --id abc123-session

# Filter by project
ccusage session --project myproject
```

### Blocks Command

Options for 5-hour billing blocks:

```bash
# Show only active block
ccusage blocks --active
ccusage blocks -a

# Show recent blocks (last 3 days)
ccusage blocks --recent
ccusage blocks -r

# Set token limit for warnings
ccusage blocks --token-limit 500000
ccusage blocks --token-limit max

# Live monitoring mode
ccusage blocks --live
ccusage blocks --live --refresh-interval 2

# Customize session length
ccusage blocks --session-length 5
```

### Statusline

Options for statusline display:

```bash
# Basic statusline
ccusage statusline

# Force offline mode
ccusage statusline --offline

# Enable caching
ccusage statusline --cache

# Custom refresh interval
ccusage statusline --refresh-interval 5
```

## JSON Output

```bash
# Print JSON output
ccusage daily --json

# Print JSON without cost fields
ccusage daily --json --no-cost

# Pipe JSON output to jq
ccusage daily --json | jq ".data[]"

# Extract specific fields
ccusage session --json | jq ".data[] | {date, cost}"
```

## Option Precedence

Options are applied in this order (highest to lowest priority):

1. **Command-line arguments** - Direct CLI options
2. **Custom config file** - Via `--config` flag
3. **Local project config** - `.ccusage/ccusage.json`
4. **User config** - `~/.config/claude/ccusage.json`
5. **Legacy config** - `~/.claude/ccusage.json`
6. **Built-in defaults**

## Examples

### Development Workflow

```bash
# Daily development check
ccusage daily --instances --breakdown

# Check specific project costs
ccusage daily --project myapp --since 20260101

# Export for reporting
ccusage monthly --json > monthly-report.json
```

### Team Collaboration

```bash
# Use team configuration
ccusage daily --config ./team-config.json

# Consistent timezone for remote team
ccusage daily --timezone UTC

# Generate shareable report
ccusage weekly --json
```

### Cost Monitoring

```bash
# Monitor active usage
ccusage blocks --active --live

# Check if approaching limits
ccusage blocks --token-limit 500000

# Historical analysis
ccusage monthly --mode calculate --breakdown
```

### Debugging Issues

```bash
# Debug configuration loading
ccusage daily --debug --config ./test-config.json

# Check pricing discrepancies
ccusage daily --debug --debug-samples 20

# Silent mode for scripts
LOG_LEVEL=0 ccusage daily --json
```

## Short Aliases

Many options have short aliases for convenience:

| Long Option   | Short | Description         |
| ------------- | ----- | ------------------- |
| `--json`      | `-j`  | JSON output         |
| `--breakdown` | `-b`  | Per-model breakdown |
| `--offline`   | `-O`  | Offline mode        |
| `--timezone`  | `-z`  | Set timezone        |
| `--instances` | `-i`  | Group by project    |
| `--project`   | `-p`  | Filter project      |
| `--active`    | `-a`  | Active block only   |
| `--recent`    | `-r`  | Recent blocks       |

## Related Documentation

- [Environment Variables](/guide/environment-variables) - Configure via environment
- [Configuration Files](/guide/config-files) - Persistent configuration
- [Cost Calculation Modes](/guide/cost-modes) - Understanding cost modes
