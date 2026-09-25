# OpenCode Source

Data source:

When `OPENCODE_DATA_DIR` is set, the adapter reads databases from that
directory. Otherwise, it uses `${XDG_DATA_HOME:-$HOME/.local/share}/opencode`:

```text
${OPENCODE_DATA_DIR}/opencode.db
${OPENCODE_DATA_DIR}/opencode-*.db
${XDG_DATA_HOME:-$HOME/.local/share}/opencode/opencode.db
${XDG_DATA_HOME:-$HOME/.local/share}/opencode/opencode-*.db
```

SQLite databases are the primary source. Legacy JSON messages under `storage/message/` are loaded as a fallback and deduplicated behind database rows.

OpenCode v2 forks copy the parent's `session_message` rows with new ids but the same `seq`. When `session_v2.fork_session_id` and `fork_boundary` resolve against the parent's rows, fork rows at or below the last copied `seq` (the boundary for `through`, the parent's last row before it for `before`) are skipped as inherited history. The fork's `session_v2` counters start at zero and hold only fork-local usage, so they remain a valid session fallback. Unresolvable boundaries and pre-`seq` schemas keep every row.

Token mapping:

- `inputTokens` <- `tokens.input`
- `outputTokens` <- `tokens.output`
- `cacheReadInputTokens` <- `tokens.cache.read`
- `cacheCreationInputTokens` <- `tokens.cache.write`

Messages may include a pre-calculated `cost` field in USD.
