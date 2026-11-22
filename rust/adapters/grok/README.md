# ccusage-adapter-grok

The Grok Build CLI adapter: it turns session `updates.jsonl` files under
`~/.grok` into the usage entries the reports render.

## Data source

Only completed turns:

```text
$GROK_HOME/   # or ~/.grok
└── sessions/
    └── <url-encoded-cwd>/
        └── <session-uuid>/
            ├── updates.jsonl  # PRIMARY (turn_completed + usage)
            └── summary.json   # optional metadata
```

Runtime root priority: a non-empty `GROK_HOME` → `~/.grok`.
Path discovery stays inside the adapter, matching Grok Build CLI's official
environment variable and default home.

In-progress turns are not counted until `turn_completed` is written. A session
killed mid-turn never gets that row, so its usage is invisible here even though
`logs/unified.jsonl` recorded the underlying requests. That log is not used as a
source: it carries no per-request model id, so its tokens cannot be priced or
attributed.

Completed turns use `agentTimestampMs` in milliseconds, then the envelope
`timestamp` in seconds. Turns with neither valid timestamp are skipped with a diagnostic.

## Token mapping

Grok records OpenAI-style usage where `inputTokens` includes cache reads and cache writes:

| Grok field                                             | ccusage field                 | Rule                                                                                          |
| ------------------------------------------------------ | ----------------------------- | --------------------------------------------------------------------------------------------- |
| `inputTokens − cachedReadTokens − cacheCreationTokens` | `input_tokens`                | cache clamped ≤ input                                                                         |
| `cachedReadTokens`                                     | `cache_read_input_tokens`     |                                                                                               |
| `outputTokens`                                         | `output_tokens`               | as recorded                                                                                   |
| `reasoningTokens`                                      | dropped                       | already inside `outputTokens`; counting it again would inflate the total                      |
| `cacheCreationTokens`                                  | `cache_creation_input_tokens` | carved out of the uncached remainder; only requests sent through Anthropic Messages report it |
| `costUsdTicks`                                         | `cost_usd`                    | fixed-point USD, one tick is 1e-10 USD                                                        |

`costUsdTicks` is the invoice cost, so `display` and the default `auto` report what
Grok billed. `calculate`, and `auto` for turns that recorded no ticks, fall back to
token × LiteLLM pricing. That fallback only approximates the invoice: a
`turn_completed` row aggregates several API requests, so it cannot reproduce Grok's
per-request long-context tiering and is priced at the base rates.

## Model display and pricing

- Display label: raw `modelUsage` key (e.g. `grok-4.5-build`)
- Pricing candidates strip trailing `-build` and try `xai/` / `x-ai/` forms

## Testing

Unit tests synthesize `updates.jsonl` fixtures (via `ccusage-test-support`)
rather than committing real session trees, which would embed absolute paths and
user prompts. Coverage spans path discovery, token split / pricing candidates,
in-file and cross-session dedupe, timestamp resolution, summary metadata, and
session report activity bounds.

```powershell
# Requires CCUSAGE_PRICING_JSON_PATH (or Nix) for the embedded LiteLLM snapshot.
cargo test -p ccusage-adapter-grok
```

## Live smoke

```powershell
cargo test -p ccusage-adapter-grok smoke_real_grok_home_loads_without_error -- --ignored --nocapture
```
