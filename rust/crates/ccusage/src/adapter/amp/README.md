# Amp Source

Data sources:

```text
amp threads list --include-archived --json
amp threads export <thread-id>
${AMP_DATA_DIR:-~/.local/share/amp}/threads/ (legacy)
```

By default, ccusage loads legacy local JSON files first, then uses the installed
and authenticated Amp CLI to discover and export server-backed threads. Remote
server snapshots older than the local history are skipped. Threads
updated after local history stopped are merged by usage identity so ccusage can
include newer messages without duplicating legacy usage or losing its credits.
Setting `AMP_DATA_DIR` explicitly selects only those local archive roots and
disables server discovery.

Usage comes from:

- `usageLedger.events[]` for token usage and credits, with `messages[].usage`
  supplying the cache creation/read breakdown per `toMessageId`. Each event's
  `tokens` object uses the legacy keys `input`, `output`, and `total`.
- `messages[].usage` directly when `usageLedger.events` is not present (current
  Amp schema). Each assistant message's `usage` object carries `model`,
  `timestamp`, and the `inputTokens`, `outputTokens`, `cacheCreationInputTokens`,
  `cacheReadInputTokens`, and `totalTokens` fields. `totalTokens` is only used
  as a fallback when the split fields are missing.

Legacy Amp ledgers also report `credits`; display credits alongside USD
estimates when the command/report supports it. Current server exports do not
include credits.

Commands:

```sh
ccusage amp daily
ccusage amp monthly
ccusage amp session
ccusage amp daily --json
ccusage amp daily --compact
```
