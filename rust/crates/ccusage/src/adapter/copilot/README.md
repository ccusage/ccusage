# GitHub Copilot CLI Source

Data source:

```text
${COPILOT_CONFIG_DIR:-~/.copilot}/session-state/<sessionId>/events.jsonl
```

The Copilot CLI writes `session-state/<uuid>/events.jsonl` automatically during each CLI session, appending a `session.shutdown` event on graceful exit.

## session-state schema

Relevant JSONL event:

- `type === "session.shutdown"` (other event types — `tool.execution_start`,
  `assistant.message`, etc. — may contain the literal string
  `"session.shutdown"` inside `arguments.command` or `content`; the parser
  requires the top-level `type` field after JSON parsing, never trusts the
  substring alone).
- `id` — per-event UUID, used as part of the dedup key
  (`shutdown:{session_id}:{event.id}:{model}` for token-bearing rows;
  `session_id` is prefixed as defense-in-depth against a future Copilot
  CLI that might scope event ids per-session).
- `timestamp` — RFC3339 with ms (`"2026-05-02T15:27:09.013Z"`).
- `data.modelMetrics` — map keyed by Copilot model id; each value carries
  `usage.{inputTokens, outputTokens, cacheReadTokens, cacheWriteTokens,
  reasoningTokens}` and `requests.{count, cost}`.

### Token mapping (session-state)

`usage.inputTokens` in this schema is inclusive of **both** `cacheReadTokens`
and `cacheWriteTokens`.

| Field on `LoadedEntry`           | Source                                                                      |
| -------------------------------- | --------------------------------------------------------------------------- |
| `input_tokens`                   | `usage.inputTokens − usage.cacheReadTokens − usage.cacheWriteTokens` (saturating) |
| `output_tokens`                  | `usage.outputTokens`                                                         |
| `cache_read_tokens`              | `usage.cacheReadTokens`                                                      |
| `cache_creation_input_tokens`    | `usage.cacheWriteTokens`                                                     |
| `reasoning_output_tokens`        | provider-dependent — see below                                              |

`data.tokenDetails` exists in the schema but is **intentionally not used as a
source**: empirical verification showed `usage.*` and `tokenDetails.*` are
independent snapshots that diverge in ~31% of resumed-session rows. We use
`usage.*` consistently across all four token fields.

### Reasoning tokens

Provider conventions differ. Verified locally across the full session-state
corpus:

| Provider                  | Convention                                              | Rule                              |
| ------------------------- | ------------------------------------------------------- | --------------------------------- |
| `claude-*` (Anthropic)    | Extended-thinking tokens included in `outputTokens`     | `reasoning_output_tokens = 0`     |
| `gpt-*` (OpenAI)          | `reasoning_tokens` is a subset of `output_tokens`       | `reasoning_output_tokens = 0`     |
| `gemini-*` (Google)       | `thoughtsTokenCount` is a separate field from output    | `reasoning_output_tokens = usage.reasoningTokens` |
| Anything else (default)   | Treated like Claude/GPT — the subset rule cannot over-count | `reasoning_output_tokens = 0` |

### Skip rule

A `modelMetrics` row is skipped when **all** of the following are true:

1. All non-reasoning token fields are zero (`input`, `output`, `cache_read`,
   `cache_write`), AND for Gemini models `reasoning_tokens` is also zero.
2. `requests.count` is zero.
3. `metrics.totalNanoAiu` is absent or zero.

Matches PR #957's `data-loader.ts` inline skip-row predicate, extended with the AIU
carve-out so that real-data credit-only rows (zero tokens, zero requests,
non-zero `totalNanoAiu`) survive. The reasoning-token clause is
provider-gated to stay symmetric with `build_session_state_entry`: for
Gemini the builder preserves `reasoning_tokens` verbatim (separate field
from output), so the skip rule counts it as usage; for OpenAI/Anthropic
the builder discards it (already subsumed into `output_tokens`), so the
skip rule ignores it to avoid surfacing all-zero phantom rows. See
`keeps_zero_token_row_when_per_model_total_nano_aiu_is_present`,
`keeps_gemini_row_with_only_reasoning_tokens`, and
`skips_non_gemini_row_with_only_reasoning_tokens` for the matching
regressions.

### Resumed sessions and dedup-key design

A single `events.jsonl` may contain multiple `session.shutdown` events (the user
re-opened the same session UUID multiple times). Each shutdown is a
**per-process snapshot** — token counts are NOT cumulative across resumes. The
parser emits one entry per `(session_id, event.id, model)` triple so the
resumed-session totals are summed correctly at report time.

The dedup-key selector chooses between two key shapes for each surviving row,
and the two shapes deliberately encode **opposite beliefs about what a duplicate
shutdown means** — because real-data shapes differ between the two paths:

- **Token-bearing / priced rows** key on `shutdown:{session_id}:{event_id}:{model}`.
  Two shutdowns with distinct event ids are treated as distinct charges (sum at
  report time), even if their token counts happen to coincide. This is correct
  because per-process snapshots commonly produce near-identical counts when a
  user resumes a similar workload, and treating them as duplicates would
  silently under-count usage. The `session_id` prefix is defense-in-depth:
  today every observed `event.id` is a globally-unique UUID, but prefixing
  with `session_id` guards against a hypothetical future Copilot CLI that
  scopes event ids per-session, which would otherwise silently collapse
  distinct sessions' rows in the loader's final `HashMap`-keyed dedup. Pinned
  by `token_bearing_rows_keep_event_id_dedup_key_not_content_based` and
  `treats_resumed_session_shutdowns_as_distinct_entries`.
- **Credit-only zero-token rows** key on
  `credit-shutdown:{session_id}:{model}:{naiu}` — **deliberately omitting
  `event_id`**. The loader's `HashMap::insert` collapse is last-wins, so
  duplicate AIU snapshots with identical `totalNanoAiu` collapse instead of
  double-counting. Pinned by
  `auto_mode_dedupes_duplicate_credit_only_rows_within_same_session` and
  `credit_only_rows_with_identical_naiu_but_distinct_event_ids_are_treated_as_duplicates`.

**The asymmetry is intentional.** Token rows trust event ids because their
data shape (per-process snapshots) makes coincidence-collision a far more
likely failure mode than missed-duplicate. Credit-only rows distrust event
ids because paired snapshots make the opposite true.

The aggregate-credit synthetic entry (emitted only when no per-model row
carries a priced billing signal — see `parse_session_state_file`'s
suppression guard) uses the same content-based key family
(`credit-aggregate:{session_id}:{nano_aiu}`) to mirror the credit-only
"paired snapshot" semantics.

### Date filter must run before dedup

The credit-only key shape above (which deliberately omits `event_id`) makes
the loader's `HashMap`-based dedup collapse **last-wins by parse order**:
two credit-only rows in the same session with the same `(model, naiu)` but
distinct timestamps collapse into whichever was inserted last. That's the
intended behaviour for paired snapshots inside an active date range — but
if the two rows straddle a `--since` / `--until` boundary, applying the
date filter **after** the dedup HashMap could keep the out-of-range row,
drop it on the filter, and silently lose the in-range row's credits
entirely.

The loader therefore applies the `--since` / `--until` filter to the
`CopilotUsageEntry` vector **before** the dedup `HashMap::insert` collapse.
Each surviving entry already satisfies the date range, so the collapse can
never pick an out-of-range winner. Token-bearing rows (event-id keyed) are
immune to this ordering bug, but the filter applies uniformly to keep the
loader-layer surface symmetric across both key families. The downstream
`filter_loaded_entries_by_date` calls in `copilot::run` and the cross-source
aggregator (`adapter::all::loader`) remain in place as defense-in-depth
no-ops once the loader has filtered, and to keep the user-facing
`LoadedEntry` surface uniform with the other adapters. Pinned by
`date_filter_runs_before_credit_only_dedup_collapse_to_preserve_in_range_row`.

## Model normalization

The adapter ships `normalize_copilot_model()` for pricing-lookup callers. The
function is applied **only inside `loader.rs::usage_entry_to_loaded`** for both
`calculate_cost_for_usage` and `missing_pricing_model_for_usage` call sites —
the entry itself keeps the raw model name so reports, JSON and `modelsUsed`
preserve source fidelity.

Rules:

1. Strip Copilot routing suffixes in this order: `-1m-internal`, `-internal`,
   `-xhigh`, `-high`, `-1m`. Required because
   `pricing::suffix_starts_with_numeric_model_version` rejects suffixes that
   begin with another digit, so without stripping the `-1m` family of variants
   no pricing entry resolves.
2. For `claude-*` names, convert version-number dots to dashes (e.g.
   `claude-opus-4.7` → `claude-opus-4-7`). Matches the canonical LiteLLM keys.
3. GPT names pass through unchanged — LiteLLM keeps them dotted.
4. Unknown prefixes pass through unchanged.

## Cost modes

| `--mode` | Behavior for Copilot                                                                                                                                                                                              |
| -------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `auto`   | True bill, billing-field-aware: AIU when present (typically post-cutover) → premium-requests × $0.04 when present (typically pre-cutover) → token-priced via LiteLLM (neither billing field). The default. Free-tier rows (`requests.cost == 0`) bill as $0. |
| `display`| Source-precomputed only: AIU → premium-requests → $0. No token-priced fallback.                                                                                                                                   |
| `calculate` / `api` | Always token-priced via LiteLLM (`normalize_copilot_model()` applied at both `calculate_cost_for_usage` and `missing_pricing_model_for_usage` call sites).                                          |

The billing-field-aware default reflects that GitHub Copilot switched billing
models on June 1, 2026: pre-cutover sessions were billed in "premium requests"
with per-model multipliers at $0.04/overage-request, and post-cutover sessions
bill in AI Units (1 AIU = 1 credit = $0.01). The Copilot CLI records each
session's own billing data, so `auto` surfaces what GitHub actually charged
you for that session.

**The dispatch keys off field presence, not the cutover date.** The
loader never compares `entry.timestamp` to June 1, 2026 — it just asks
"did this row ship `totalNanoAiu`?" and picks AIU billing if so. The cutover
date is the *why* behind the two billing columns existing in the wire
schema; it is not a branching condition in the code. This shape is more
robust than a date check: it correctly handles a pre-cutover session that
happens to ship AIU (rare but possible if GitHub ever backfills) and
generalises to a hypothetical future third billing channel without a
schema-version migration.

`PREMIUM_REQUEST_COST_USD = $0.04` and `AI_CREDIT_COST_USD = $0.01` are
GitHub's published external pricing rates, not contracts encoded in the
CLI's wire format. If GitHub adjusts either rate, `auto`/`display` will
mis-bill until the constants are updated. The mitigation users have today
is `--mode api` (alias of `calculate`): that path bills from token counts
via LiteLLM and does not depend on either constant.

`LoadedEntry.credits` is populated independently of `--mode` whenever the
source carried `totalNanoAiu`, so JSON consumers see the raw credit count
regardless of mode selection.

The `missing_pricing_model` warning never fires for the AIU and premium-request
branches of `auto`/`display` because they don't consult LiteLLM — those
branches return `None` directly. The shared
`cost.rs::missing_pricing_model_for_usage` early-return remains intact so other
adapters still see legitimate missing-pricing warnings when their token-pricing
fallback can't resolve a model.

## Environment variables

| Variable             | Effect                                                          |
| -------------------- | --------------------------------------------------------------- |
| `COPILOT_CONFIG_DIR` | Override the base Copilot directory (defaults to `~/.copilot`). |

## Legacy OpenTelemetry environment variables

ccusage no longer reads OpenTelemetry exports — the production loader uses
the session-state source described above directly. The legacy OTel
environment variables (`COPILOT_OTEL_FILE_EXPORTER_PATH`, `COPILOT_OTEL_DEDUP`,
`COPILOT_PREFER_OTEL`) are inert; if any of them are set when `ccusage copilot`
runs, a one-shot stderr warning explains they are ignored. Unset them, or set
`LOG_LEVEL=0`, to silence the warning.
