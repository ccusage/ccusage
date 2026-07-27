# Amp Source

Data source:

```text
${AMP_DATA_DIR:-~/.local/share/amp}/threads/
```

Each thread is a JSON file named `T-{uuid}.json`.

Usage comes from:

- `usageLedger.events[]` for token usage and credits, with `messages[].usage`
  supplying the cache creation/read breakdown per `toMessageId`. Each event's
  `tokens` object uses the legacy keys `input`, `output`, and `total`.
- `messages[].usage` directly when `usageLedger.events` is not present (current
  Amp schema). Each assistant message's `usage` object carries `model`,
  `timestamp`, and the `inputTokens`, `outputTokens`, `cacheCreationInputTokens`,
  `cacheReadInputTokens`, and `totalTokens` fields. `totalTokens` is only used
  as a fallback when the split fields are missing.

Amp also reports `credits`; display credits alongside USD estimates when the command/report supports it.

Commands:

```sh
ccusage amp daily
ccusage amp monthly
ccusage amp session
ccusage amp daily --json
ccusage amp daily --compact
```
