# Command-Line Options

ccusage provides extensive command-line options to customize its behavior. These options take precedence over configuration files.

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

Both bounds accept `YYYY-MM-DD` or `YYYYMMDD` and are inclusive. Any other spelling, or a value that is not a real calendar date such as `2026-02-30`, is rejected with a non-zero exit code instead of silently changing which rows the report keeps. The same check applies to `since` and `until` in a [configuration file](/guide/config-files). A window whose `--since` date is later than its `--until` date is rejected the same way, whether the bounds come from flags, a configuration file, or a mix of both. `statusline` ignores report date filters, including configured defaults, but configuration files are still validated in full.

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

The count is inclusive of the current period, so `--last 2` on a daily report covers yesterday and today. The window also ends with the current period, so records dated after the end of today, this week, or this month, such as logs merged from a machine whose clock runs ahead, are left out. Weeks start on the same day the report buckets by: Monday by default and Sunday for `ccusage claude weekly`, unless `commands.weekly.startOfWeek` in a [configuration file](/guide/config-files) or `--start-of-week` on `ccusage weekly` or `ccusage claude weekly` sets another day, with the flag taking precedence.

`--last` works on every daily, weekly, and monthly report, including the per-agent ones such as `ccusage codex daily --last 1`. It is not available on `session`, `blocks`, or `statusline`, which have no calendar period, and it cannot be combined with `--since`, `--until`, or `--sections`. A `since` or `until` set in a [configuration file](/guide/config-files) is not a conflict: `--last` replaces that window.

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
# Oldest first (default)
ccusage daily --order asc

# Newest first
ccusage daily --order desc
```

Unified session reports are the exception: without an order option they list the most expensive session first. See [Session Usage](/guide/session-reports#sorting).

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

The value must be `UTC`, another IANA timezone name, or `local` for the system timezone. An unknown name such as `Not/AZone` is rejected with a non-zero exit code instead of silently falling back to the system timezone, which can group usage under the wrong date. The same check applies to `timezone` in a [configuration file](/guide/config-files).

#### Timezone Effect

The timezone affects how usage is grouped by date. For example, usage at 11 PM UTC on January 1st would appear on:

- **January 1st** when `--timezone UTC`
- **January 1st** when `--timezone America/New_York` (6 PM EST)
- **January 2nd** when `--timezone Asia/Tokyo` (8 AM JST next day)

### Debug Options

Get detailed debugging information:

```bash
# Debug mode - show skipped input and loader details on stderr
ccusage daily --debug
```

Skipped-input warnings summarize files or usage records left out of a report;
healthy input continues to count. `--debug` adds a bounded sample of file paths
and record locations, including usage lines that could not be decoded. JSON
decoding details show the error category and location without echoing log
contents. These diagnostics go to stderr, keeping JSON output on stdout.

A log that ends part-way through a record is treated as still being written
when the file changes while ccusage reads it, and only `--debug` mentions it;
the next run reads the record whole. A log that ends part-way through a record
and does not change during the read, for example after an agent crash, is
reported as invalid data however recently it was modified.

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
```

`--sections` accepts a comma-separated list of `daily`, `weekly`, `monthly`, and `session`. The invoked report section is always included. For table output, each requested section is printed as a separate table. `--by-agent` is JSON-only; session rows are already per-agent.

### Claude Project Options

`--project <name>` overrides the configured project on Claude daily, weekly, monthly, session, and blocks reports. The top-level `blocks` and `session` commands also accept it. In unified session reports it filters Claude Code usage. A Claude session lookup with `--id` searches across projects.

`--instances` and `--project-aliases` are options for `ccusage claude daily`.

```bash
# Group by project
ccusage claude daily --instances
ccusage claude daily -i

# Filter to specific project
ccusage claude weekly --project myproject
ccusage claude daily -p myproject

# Combine project filtering
ccusage claude daily --instances --project myproject

# Rename projects in the table
ccusage claude daily --instances --project-aliases "uuid-project=Frontend App"
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
# Look up one session by ID
ccusage claude session --id abc123-session
ccusage codex session --id 01a0bb8c-c7c0-7630-9d82-860b875930f0
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
ccusage daily --json | jq ".daily[]"

# Extract specific fields
ccusage session --json | jq ".session[] | {period, agent, totalCost}"
```

## Option Precedence

Command-line arguments override the [configuration file](/guide/config-files#configuration-priority), which overrides the built-in defaults. ccusage reads one configuration file: the `--config` file when given, otherwise the first of `.ccusage/ccusage.json` and the user config files that exists and holds a JSON object. Settings from different files are never merged.

## Examples

### Development Workflow

```bash
# Daily development check
ccusage claude daily --instances --breakdown

# Check specific project costs
ccusage claude daily --project myapp --since 20260101

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
ccusage blocks --active

# Check if approaching limits
ccusage blocks --token-limit 500000

# Historical analysis
ccusage monthly --mode calculate --breakdown
```

### Debugging Issues

```bash
# Debug a run with a test configuration
ccusage daily --debug --config ./test-config.json

# Silent mode for scripts
LOG_LEVEL=0 ccusage daily --json
```

## Short Aliases

Many options have short aliases for convenience:

| Long Option   | Short | Description                               |
| ------------- | ----- | ----------------------------------------- |
| `--json`      | `-j`  | JSON output                               |
| `--breakdown` | `-b`  | Per-model breakdown                       |
| `--offline`   | `-O`  | Offline mode                              |
| `--timezone`  | `-z`  | Set timezone                              |
| `--instances` | `-i`  | Group by project (`ccusage claude daily`) |
| `--project`   | `-p`  | Filter Claude Code usage by project       |
| `--id`        | `-i`  | Session lookup (`session`)                |
| `--active`    | `-a`  | Active block only (`blocks`)              |
| `--recent`    | `-r`  | Recent blocks (`blocks`)                  |

## Related Documentation

- [Environment Variables](/guide/environment-variables) - Configure via environment
- [Configuration Files](/guide/config-files) - Persistent configuration
- [Cost Calculation Modes](/guide/cost-modes) - Understanding cost modes
