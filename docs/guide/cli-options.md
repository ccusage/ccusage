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

### Live Only Mode

Show only usage from log files that still exist on disk, excluding spend retained from deleted source logs:

```bash
# Show only current log file usage
ccusage daily --live-only
ccusage claude daily --live-only
```

This flag excludes data that was recovered from deleted source logs and stored in the spend ledger. The fast on-disk cache stays active.

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

### Clear Cache Command

Clear the on-disk cache. You can clear everything or target a single agent:

```bash
# Clear the entire cache
ccusage clear-cache
ccusage clear-cache all

# Clear only one agent's cached data
ccusage clear-cache claude
ccusage clear-cache opencode
```

Output messages:

- `Cleared the cache.` — the whole cache was removed.
- `Cleared the <agent> cache.` — that agent's cached data was removed.
- `<agent> has no on-disk cache to clear.` — that agent currently had nothing cached.

See [Clear Cache](/guide/clear-cache) for details on what is cached and the ledger caveat.

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
- [Clear Cache](/guide/clear-cache) - Manage the on-disk cache
