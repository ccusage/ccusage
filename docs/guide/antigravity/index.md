# Antigravity

> Antigravity support reads local conversation SQLite databases.

## Quick Start

```sh
# Daily usage report
ccusage antigravity daily

# Monthly usage report
ccusage antigravity monthly

# Session usage report
ccusage antigravity session
```

## Data Source

The CLI reads Antigravity conversation databases from:
- `~/.gemini/antigravity/conversations/*.db`
- `~/.gemini/antigravity-ide/conversations/*.db`
- `~/.gemini/antigravity-cli/conversations/*.db`
- `~/.gemini/antigravity-backup/conversations/*.db`

You can override the discovery path using `ANTIGRAVITY_DATA_DIR` (supports single root or comma-separated list of roots):

```sh
ANTIGRAVITY_DATA_DIR="$HOME/.gemini/antigravity" ccusage antigravity daily
```

## Supported Reports

| Command                     | Description                     | Documentation Reference                  |
| --------------------------- | ------------------------------- | ---------------------------------------- |
| `ccusage antigravity daily`   | Group usage by day              | [Daily Usage](/guide/daily-reports)     |
| `ccusage antigravity monthly` | Group usage by month            | [Monthly Usage](/guide/monthly-reports) |
| `ccusage antigravity session` | Group usage by session UUID     | [Session Usage](/guide/session-reports) |
