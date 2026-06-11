# Clear Cache

`ccusage` keeps a small on-disk cache so repeat runs are fast. The `clear-cache`
command removes that cache. Clearing is safe — your current usage data is rebuilt
from your source logs on the next run.

## Usage

<!-- eslint-skip -->

```bash
# Clear the entire cache
ccusage clear-cache

# Same as above
ccusage clear-cache all

# Clear only one agent's cached data
ccusage clear-cache claude
ccusage clear-cache opencode
```

Output:

- `Cleared the cache.` — the whole cache was removed.
- `Cleared the <agent> cache.` — that agent's cached data was removed.
- `<agent> has no on-disk cache to clear.` — that agent currently had nothing cached.

`<agent>` accepts any supported agent name (for example `claude`, `codex`,
`opencode`, `amp`, `droid`, `codebuff`, `hermes`, `pi`, `goose`, `kilo`,
`copilot`, `gemini`, `kimi`, `qwen`, `openclaw`). Some agents may not keep a
cache yet, in which case the command reports nothing to clear.

## Where the cache lives

A single SQLite database, `cache.db`, under:

- `$XDG_CACHE_HOME/ccusage` when `XDG_CACHE_HOME` is set, otherwise
- `~/.cache/ccusage`

Two WAL sidecar files (`cache.db-wal`, `cache.db-shm`) sit alongside it.

## What is cached

| Data                                 | Rebuildable?            |
| ------------------------------------ | ----------------------- |
| Parsed usage entries per source file | Yes                     |
| OpenCode database rows               | Yes                     |
| LiteLLM model pricing                | Yes                     |
| Spend ledger                         | No — see the note below |

The cache is a performance optimization. A cache failure never breaks a command;
`ccusage` falls back to reading your source logs directly.

## When to clear

- Numbers look stale or wrong after an upgrade.
- You want to reclaim the disk space the cache uses.
- You are troubleshooting and want a clean rebuild.

::: warning Ledger note
A full clear also discards the spend **ledger**, which re-emits cost for source
files you have since deleted. Spend that came only from logs no longer on disk
cannot be rebuilt. Clear a single agent (`ccusage clear-cache <agent>`) to limit
this to one agent.
:::

## See also

- [Configuration](./configuration) — what `ccusage` caches and where.
- [Environment variables](./environment-variables) — cache directory location.
