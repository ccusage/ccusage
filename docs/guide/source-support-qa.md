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

::: details Why is Antigravity CLI not supported?
Antigravity CLI is separate from Gemini CLI. The Antigravity CLI binary is exposed as `agy`, and it stores state under `~/.gemini/antigravity-cli/`.

The current local data has conversation files such as `conversations/<conversation-id>.pb`, plus lightweight history and cache JSON files. The `.pb` files are opaque binary payloads and do not expose readable token usage, model usage, or per-turn accounting without Antigravity's private schema and storage semantics.

The CLI log files include operational events such as conversation creation, streaming, prompt length, auth, and model configuration messages. They do not include input, output, cache, or reasoning token counts. Quota-oriented tools can inspect remaining Antigravity quota, but quota snapshots are not the same as historical per-session token usage.

Because the local files do not expose the token accounting needed for ccusage reports, Antigravity CLI is not supported right now.
:::

::: details Why is Devin CLI not supported?
Devin CLI usage information appears to live in Devin's cloud service rather than in a local usage log that ccusage can read. The locally available data did not provide direct access to historical token usage or costs.

ccusage is a local, read-only analyzer. It does not scrape private cloud services or depend on undocumented authenticated APIs for user usage history. If Devin adds a local export with timestamps, sessions, models, and token counts, support can be revisited.
:::

## Previously Unsupported, Now Supported

::: details Grok Build CLI
Earlier investigations looked at local Grok data that did not expose usable token
accounting (for example SQLite without per-turn usage). Grok Build CLI is now
supported: ccusage reads completed turns from
`${GROK_HOME:-~/.grok}/sessions/**/updates.jsonl` (`sessionUpdate == "turn_completed"`
with `usage` / `modelUsage`). Costs are LiteLLM token estimates; Grok
`costUsdTicks` are not used as invoice USD. In-progress turns count only after
`turn_completed`. See [Grok Build CLI](/guide/grok/).
:::

## Can These Be Added Later?

Yes. Open an issue if a tool starts writing local usage data with token counts or exposes an official export. Useful examples include:

- A sample redacted log file
- The default data directory
- A description of which fields represent input, output, cache, reasoning, model, timestamp, and session ID
- Notes about whether costs are recorded or should be calculated from model pricing

Please do not share secrets, API keys, OAuth tokens, raw private prompts, or full conversation transcripts.
