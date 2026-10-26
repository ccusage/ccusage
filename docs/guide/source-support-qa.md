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

## Sources Included Automatically

::: details Oh My Pi (OMP)
Oh My Pi writes usage in the Pi session format. Unified reports discover its default sessions automatically and include the data as agent `omp`, with raw model names prefixed by `[omp] `. There is no `ccusage omp` command; use a unified report such as `ccusage daily`. The focused `ccusage pi ...` command remains specific to pi-agent. Existing `pi.stores[]` entries are still supported, including an explicit store named `omp`; that explicit entry suppresses automatic OMP discovery.

The default root is `~/.omp/agent/sessions`. `PI_CONFIG_DIR` selects its config root; `PI_CODING_AGENT_DIR` adds another agent root while default, profile, and migrated `$XDG_DATA_HOME/omp` roots remain discoverable on Linux and macOS. See [Pi-agent and Oh My Pi](/guide/pi/) for path rules.

The adapter reads local JSONL session files only, including billable top-level `model_usage` entries. It handles the title slot before the session header, the `activeLeafId` branch pointer, and `parentSession` references stored as opaque IDs or paths. Compaction `tokensBefore` is metadata rather than billable usage. SQLite or Redis-backed session storage and compressed `.jsonl.gz` archives are outside the supported input format. See [Pi-agent and Oh My Pi](/guide/pi/) and the [Oh My Pi session format reference](https://github.com/can1357/oh-my-pi/blob/e0996aad2cf487c82a7ba00f46172600fde48528/docs/session.md).
:::

## Unsupported Sources Investigated

::: details Why is Devin CLI not supported?
Devin CLI usage information appears to live in Devin's cloud service rather than in a local usage log that ccusage can read. The locally available data did not provide direct access to historical token usage or costs.

ccusage is a local, read-only analyzer. It does not scrape private cloud services or depend on undocumented authenticated APIs for user usage history. If Devin adds a local export with timestamps, sessions, models, and token counts, support can be revisited.
:::

## Previously Unsupported, Now Supported

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
