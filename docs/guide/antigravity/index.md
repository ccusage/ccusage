# Antigravity Data Source (Experimental)

Antigravity support is experimental. ccusage can read Antigravity CLI usage from the per-conversation SQLite databases it writes locally, opening them read-only.

Read the [accuracy notes](#accuracy-notes) before relying on these numbers. Antigravity stores usage as protobuf with no published schema, and some of its recording paths could not be verified against real data yet.

## Quick Start

```bash
ccusage antigravity daily
ccusage antigravity monthly
ccusage antigravity session
```

Antigravity is also included in unified reports when supported databases are detected:

```bash
ccusage daily
```

## Data Locations

By default, ccusage reads every conversation database under:

```text
~/.gemini/antigravity-cli/conversations/*.db
```

Antigravity is a Gemini-family tool, so its CLI state lives under the shared `.gemini` directory rather than a directory of its own.

Set `ANTIGRAVITY_DATA_DIR` when Antigravity is stored somewhere else:

```bash
ANTIGRAVITY_DATA_DIR="/path/to/antigravity-cli" ccusage antigravity daily
```

With `ANTIGRAVITY_DATA_DIR` set, ccusage reads:

```text
$ANTIGRAVITY_DATA_DIR/conversations/*.db
```

Each conversation is a separate database, and a session in these reports is one conversation.

## Token Mapping

Antigravity records one `ModelUsageStats` message per model invocation. ccusage reads:

| Antigravity field                                   | ccusage field                                    |
| --------------------------------------------------- | ------------------------------------------------ |
| `input_tokens`                                      | Input tokens                                     |
| `output_tokens`                                     | Output tokens                                    |
| `cache_write_tokens`                                | Cache create tokens                              |
| `cache_read_tokens`                                 | Cache read tokens                                |
| `thinking_output_tokens` + `response_output_tokens` | Output tokens, when the recorded total is absent |
| `response_model`                                    | Model                                            |

`input_tokens` and `cache_read_tokens` are disjoint in the source, matching how ccusage reports them, so a cache read is never also counted as input. `output_tokens` already includes thinking tokens, so reasoning is not added on top of it.

Usage is read from two tables per conversation. `steps` is the primary source and holds one row per invocation, including background calls. `gen_metadata` is the only place a human-readable model name appears, and it also keeps usage visible for a step Antigravity has since pruned.

## Cost Calculation

Antigravity leaves its `model_cost`, `credit_cost` and `consumed_credits` fields unset on consumer accounts, so there is no recorded cost to read. ccusage estimates cost from token counts and LiteLLM pricing for the recorded model name.

That figure is the equivalent public API cost, not an amount you were billed. Antigravity is quota-based, so the actual charge for this usage may well be zero.

`--mode display` therefore reports no cost, because there is nothing precomputed to display:

```bash
ccusage antigravity daily --mode calculate
```

Use `--offline` to rely on cached pricing data:

```bash
ccusage antigravity daily --offline
```

The embedded offline snapshot covers Anthropic and OpenAI models. Gemini rates are only available from a runtime pricing fetch, so `--offline` reports no cost for Gemini models.

## Accuracy Notes

Antigravity ships no `.proto` files, so ccusage decodes its stored blobs by protobuf field number. Those numbers were recovered from the schema descriptors embedded in the Antigravity binary rather than guessed from the data, and Antigravity has to read the same blobs back after an upgrade, so it cannot renumber a field without breaking its own persistence.

Within those constraints, the following limitations apply:

- **Unnamed models cannot be priced.** Antigravity records the model as a numeric id, and background invocations get no `gen_metadata` row to name them. These appear as `antigravity-model-<id>`, and ccusage warns that pricing is missing for them. Their tokens are still counted, so token totals stay complete while the cost total is understated.
- **Cost is an estimate, never a billed amount.** See [Cost Calculation](#cost-calculation).
- **Retries are handled by design but unverified.** Failed attempts consume tokens and are recorded separately from the attempt that succeeded. ccusage collects both and deduplicates on the server-assigned response id, which is correct whether or not Antigravity repeats the successful attempt in its retry list — but this has not yet been checked against a conversation that actually retried.
- **Subagents and model comparisons are unverified.** Sub-conversations get their own database, so reading every database is what makes them visible, and the same response-id dedupe prevents double counting a call a parent also recorded. Neither path has been exercised against real data.
- **Historical totals depend on retention.** Antigravity trims what it keeps per conversation. If it prunes rows for an older day, that day's total will shrink; ccusage can only report what is still on disk.

## Troubleshooting

If no Antigravity data appears, check that conversation databases exist under the default path or set `ANTIGRAVITY_DATA_DIR`.

```bash
ANTIGRAVITY_DATA_DIR="/path/to/antigravity-cli" ccusage antigravity session --json
```

## Related Guides

- [All Reports](/guide/all-reports) for unified multi-source reports
- [Gemini CLI Data Source](/guide/gemini/) for the other Gemini-family source
- [Environment Variables](/guide/environment-variables) for the full list of path overrides
- [Cost Modes](/guide/cost-modes) for how `--mode` changes cost reporting
