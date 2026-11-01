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

The Claude Agent provider (`claude-agent`) runs Claude Code, which also writes its own transcript. Its v2 rows record that Claude Code session as `content[].native.sessionID`, which `load_entries_with_native_sessions` returns with each entry. Unified reports drop a row when the Claude source counted main-thread usage of that session on the row's date, in the Claude source's timezone and date range, so the usage is counted once and priced by the resolved model ID rather than the `opus`/`sonnet` alias OpenCode stores. The match is per session day, not per turn: `native.messageID` is the transcript line `uuid`, and one OpenCode row sums several API calls, so rows do not map one-to-one to Claude entries. This relies on the transcript holding every main-thread turn OpenCode records for that session; it also holds turns OpenCode drops, such as interrupted steps saved with `tokens: null`. A row is dated by its turn's start while Claude dates each call, so a turn that crosses midnight in the Claude source's timezone can be counted by both sources when Claude has no usage of that session on the earlier day. Only v2 `session_message` rows can carry the link: OpenCode added the provider after moving to SQLite v2, so legacy JSON messages and the v1 `message` table hold no `claude-agent` rows. `ccusage opencode` keeps every row.

Token mapping:

- `inputTokens` <- `tokens.input`
- `outputTokens` <- `tokens.output`
- `cacheReadInputTokens` <- `tokens.cache.read`
- `cacheCreationInputTokens` <- `tokens.cache.write`

Messages may include a pre-calculated `cost` field in USD.
