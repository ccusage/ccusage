# Source Support Q&A

ccusage only supports a coding agent when it can read local usage records with enough information to produce accurate reports. At minimum, a source needs local timestamps, session identity, model identity, and token counts or recorded costs that can be mapped to token usage.

If a tool stores only prompts, transcripts, quota percentages, or opaque cloud state, ccusage does not estimate token usage from text length. That would make daily, monthly, session, and cost reports look precise while being based on guesses.

## What Makes a Source Supportable?

A source is a good fit when its local files include most of the following:

- Per-message or per-turn token counts
- Input and output token counts, with cache and reasoning tokens when available
- Model identifiers for pricing
- Timestamps for date filtering and grouping
- Session or conversation identifiers
- Stable local file formats such as JSONL, SQLite tables, or structured telemetry exports

Local transcript text alone is not enough. A transcript can be useful for debugging, but it does not reveal tokenizer behavior, hidden system context, cached input, tool-call overhead, or provider-side accounting.

## Unsupported Sources Investigated

::: details Why is Devin CLI not supported?
Devin CLI usage information appears to live in Devin's cloud service rather than in a local usage log that ccusage can read. The locally available data did not provide direct access to historical token usage or costs.

ccusage is a local, read-only analyzer. It does not scrape private cloud services or depend on undocumented authenticated APIs for user usage history. If Devin adds a local export with timestamps, sessions, models, and token counts, support can be revisited.
:::

## Previously Unsupported, Now Supported

| Source    | Verified basis                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             | Limits                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| --------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Muse Code | [Meta's deterministic replay guide](https://dev.meta.ai/docs/cookbook/deterministic-replay?project_id=1661600634933790&team_id=2096920474558192) documents session JSONL under `${XDG_DATA_HOME:-$HOME/.local/share}/muse/sessions`; Meta's [model pricing](https://dev.meta.ai/docs/pricing-rate-limits) and [prompt-caching documentation](https://dev.meta.ai/docs/prompt-caching) support exact Muse Spark pricing and the Meta Model API cache-read mapping. The anonymised [issue #1694 excerpt](https://github.com/ccusage/ccusage/issues/1694) is preserved unchanged, but is rejected because it lacks a top-level record ID. Synthetic complete records with stable IDs drive aggregation tests. | Eligibility is limited to explicitly Meta-routed Muse Spark completions with complete direct token fields, valid positive microsecond timestamps, and zero cache writes. Records without both `stream.id` and record `id`, or with conflicting copies, are excluded. No full real-session capture or provider-level retry identity is verified; retries written with new identities may still be double counted. Windows roots and alternate macOS roots are also unverified. |

This evidence supports reading Muse's documented XDG/home data root. `MUSE_DATA_DIR` accepts one or more comma-separated data roots, each containing a `sessions/` tree. The path and sample record are documented or observed, but the eligibility rules intentionally reject incomplete or ambiguously routed records; the excerpt does not prove that such records can be counted reliably.

::: details Antigravity
Antigravity is now supported as its own source. ccusage reads the local
`gen_metadata` SQLite databases, keeps Antigravity separate from Gemini CLI,
and maps the input, cache-read, visible-output, and thinking-token buckets into
the standard report shape. See [Antigravity](/guide/antigravity/).
:::

::: details Grok Build CLI
Earlier investigations looked at local Grok data that did not expose usable token
accounting (for example SQLite without per-turn usage). Grok Build CLI is now
supported: ccusage reads completed turns from
`${GROK_HOME:-~/.grok}/sessions/**/updates.jsonl` (`sessionUpdate == "turn_completed"`
with `usage` / `modelUsage`). Costs come from Grok's own `costUsdTicks`, with
LiteLLM estimates as the fallback. In-progress turns count only after
`turn_completed`. See [Grok Build CLI](/guide/grok/).
:::

## Can These Be Added Later?

Yes. Open an issue if a tool starts writing local usage data with token counts or exposes an official export. Useful examples include:

- A sample redacted log file
- The default data directory
- A description of which fields represent input, output, cache, reasoning, model, timestamp, and session ID
- Notes about whether costs are recorded or should be calculated from model pricing

Please do not share secrets, API keys, OAuth tokens, raw private prompts, or full conversation transcripts.
