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

SQLite databases are the primary source. Legacy JSON messages under `storage/message/` are loaded as a fallback and deduplicated behind database rows. Token mapping:

- `inputTokens` <- `tokens.input`
- `outputTokens` <- `tokens.output`
- `cacheReadInputTokens` <- `tokens.cache.read`
- `cacheCreationInputTokens` <- `tokens.cache.write`

Messages may include a pre-calculated `cost` field in USD.
