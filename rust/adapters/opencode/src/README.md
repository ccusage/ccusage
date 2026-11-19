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

The Claude Agent provider (`claude-agent`) runs Claude Code, which also writes its own transcript. Its v2 rows record that Claude Code session as `content[].native.sessionID`. `native.messageID` is the transcript line `uuid`; one OpenCode row sums several API calls, so rows do not map one-to-one to Claude entries. Only v2 `session_message` rows carry the link: OpenCode added the provider after moving to SQLite v2, so legacy JSON messages and the v1 `message` table hold no `claude-agent` rows.

Unified reports and `ccusage opencode` leave linked turns to Claude when that native session has main-thread usage within the Claude source's date range, widened by one calendar day for claim discovery. Claude still reports only calls within the original range, on each call's own date. If a linked turn starts on the `--until` date and crosses midnight, claim discovery can exclude the OpenCode row while Claude omits calls after midnight. Linked rows outside the Claude source's configured range remain in OpenCode. Session matching includes resumed or forked transcript copies that Claude deduplicates and uses the recorded session ID rather than the file name. This relies on the transcript holding every main-thread turn OpenCode records for that session, including interrupted steps OpenCode saves with `tokens: null`. Usage is priced by Claude's resolved model ID rather than OpenCode's `opus`/`sonnet` alias. `--include-claude-agent` keeps every OpenCode row.

Token mapping:

- `inputTokens` <- `tokens.input`
- `outputTokens` <- `tokens.output`
- `cacheReadInputTokens` <- `tokens.cache.read`
- `cacheCreationInputTokens` <- `tokens.cache.write`

Messages may include a pre-calculated `cost` field in USD.
