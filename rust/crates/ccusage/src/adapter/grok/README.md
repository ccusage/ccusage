# Grok Build CLI adapter

Reads local Grok Build CLI session usage from `~/.grok/sessions/**/updates.jsonl`.

## Data source

| Field | Value |
| --- | --- |
| Default home | `~/.grok` |
| Env override | `GROK_HOME` (comma-separated roots) |
| CLI override | `--grok-path` (comma-separated roots) |
| Config | `grok.defaults.grokPath` / per-command `grokPath` |
| Files | `sessions/**/updates.jsonl` (+ optional sibling `summary.json`) |

Only `sessionUpdate == "turn_completed"` records with billable token counts load.
Per-model maps come from `usage.modelUsage`; each model becomes one `LoadedEntry`.

Token mapping:

| Grok field | ccusage field |
| --- | --- |
| `inputTokens` | `input_tokens` |
| `outputTokens` | `output_tokens` (display column) |
| `cachedReadTokens` | `cache_read_input_tokens` |
| `reasoningTokens` | `extra_total_tokens` (totals) and billed at **output** rate for cost |

Display model labels use a `[grok]` prefix. Pricing candidates try the raw model
id, `xai/<model>`, and `x-ai/<model>`.

## Parent / child sessions (OQ1)

Live sampling (2026-07-13):

- Nested `…/<session>/subagents/<child-id>/` directories currently hold only
  `meta.json` pointers — **no** nested `updates.jsonl`.
- Child agent work is stored as **sibling** session directories under the same
  project path (their own `updates.jsonl`).
- There was no evidence that parent `turn_completed.usage` embeds child API
  usage in a way that would double-count when both trees are loaded.

**Rule:** load every session directory's `turn_completed` usage (all
`updates.jsonl` under `sessions/`). Do not special-case `subagents/` unless
future schema evidence shows nested usage files that already appear on the
parent turn.

## Dedup

Prefer `_meta.eventId` + model. Fallback: session id + timestamp + model + token
tuple.

## Out of scope

- Cloud xAI billing APIs
- Token estimation from `chat_history.jsonl` text
- Headless streams that never write `updates.jsonl`
