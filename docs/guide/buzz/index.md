# Buzz Data Source

ccusage can read buzz-agent token usage from its local NIP-AM archive. Buzz is a Nostr-based AI agent coordination platform; its harness emits per-turn usage metrics (kind 44200 events) that are stored as decrypted plaintext in a local SQLite archive.

## Quick Start

```bash
ccusage buzz daily
ccusage buzz monthly
ccusage buzz session
```

Buzz is also included in unified reports when the archive is detected:

```bash
ccusage daily
```

## Data Location

By default, ccusage reads:

```text
~/.buzz/archive/archive.db
```

Two environment overrides are supported:

```bash
# Point directly at the archive file:
BUZZ_ARCHIVE_PATH="/path/to/archive.db" ccusage buzz daily

# Point at a root directory (ccusage joins archive/archive.db):
BUZZ_PATH_ROOT="/path/to/buzz" ccusage buzz daily
```

## Token Mapping

ccusage reads kind 44200 events from the `archived_events` table. Each row's `raw_json` field contains a decrypted `AgentTurnMetricPayload`:

| Payload field                                      | ccusage field  |
| -------------------------------------------------- | -------------- |
| `turn.inputTokens` (when `turn` is present)        | Input tokens   |
| `turn.outputTokens` (when `turn` is present)       | Output tokens  |
| `cumulative.inputTokens` (only when `turnSeq == 1` and `turn` is null) | Input tokens |
| `cumulative.outputTokens` (only when `turnSeq == 1` and `turn` is null) | Output tokens |
| `model`                                            | Model          |
| `sessionId`                                        | Session ID     |
| `timestamp`                                        | Date / time    |

Rows where `turn` is null and `turnSeq > 1` are skipped: using cumulative totals on later turns would double-count the full session history. Cache read/write columns are not present in buzz-agent frames and are reported as zero.

## Cost Calculation

Buzz-agent frames do not store a recorded USD cost. ccusage estimates cost from token counts using LiteLLM pricing. Model names like `goose-claude-4-6-sonnet` and `databricks-claude-opus-4-6` resolve through ccusage's fuzzy pricing matcher to real Anthropic rates.

Use `--offline` to rely on cached pricing data:

```bash
ccusage buzz daily --offline
```

## Related Guides

- [Goose](../goose/index.md)
- [All-agent reports](../../all-reports.md)
