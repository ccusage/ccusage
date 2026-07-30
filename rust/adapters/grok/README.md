# ccusage-adapter-grok

The Grok Build CLI adapter: it turns session `updates.jsonl` files under
`~/.grok` into the usage entries the reports render.

## Owns

- `loader.rs` — reading sessions, progress, global dedupe, `has_data`
- `parser.rs` — `turn_completed` admission, token split, pricing candidates
- `paths.rs` — root resolution and `sessions/**/updates.jsonl` discovery
- `report.rs` — daily / monthly / session summary shapes

Anything that is not specific to this source belongs in `ccusage-core` or
`ccusage-adapter-common` instead.

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

In-progress turns are not counted until `turn_completed` is written.
`logs/unified.jsonl` is not used in v1.

## Token mapping

Grok records OpenAI-style usage where `inputTokens` includes cache:

| Grok field | ccusage field | Rule |
| --- | --- | --- |
| `inputTokens − cachedReadTokens` | `input_tokens` | cache clamped ≤ input |
| `cachedReadTokens` | `cache_read_input_tokens` | |
| `outputTokens` | `output_tokens` | as recorded |
| `reasoningTokens` | `extra_total_tokens` only | **not** billed separately |
| — | `cache_creation_input_tokens` | always `0` |
| `costUsdTicks` | ignored | `cost_usd` always unset |

Costs are token × LiteLLM pricing estimates. Display mode shows `$0` for Grok.

## Model display and pricing

- Display label: raw `modelUsage` key (e.g. `grok-4.5-build`)
- Pricing candidates strip trailing `-build` and try `xai/` / `x-ai/` forms

## Public surface

- `loader::load_entries`
- `loader::has_data`
- `report::report_from_rows`
- `report::summarize_entries`
- `run`

## Depends on

- `ccusage-adapter-common`
- `ccusage-core`
- `jiff`
- `serde`
- `serde_json`

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
