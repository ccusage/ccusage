# Skill Attribution + ccusage Fork — Design Spec

**Date:** 2026-06-18
**Status:** approved design, pre-plan

## Goal

A Claude Code usage analyzer that attributes token spend to the *skills, subagents, and context* that caused it — not just raw daily/session cost. Answers "where did my tokens actually go inside Claude Code" (the `/usage` characteristic breakdown + true per-skill cost), built on ccusage's proven multi-agent log infrastructure.

## Approach

**Fork ccusage** (MIT, Rust). Add a Claude-only content-analytics command on top of its existing discovery / dedup / pricing / terminal-output infrastructure. Claude adapter first; generalize to other adapters later (skills are a portable spec, so cross-agent is the eventual target — out of scope for v1).

Rejected: Python rewrite (would reimplement ccusage's 14-adapter discovery + dedup + pricing from scratch — the multi-agent expertise lives only in ccusage); pairing with ccusage at runtime (its JSON is aggregate and lacks the record-level fields we need — no usable seam); building on cc-usage-monitor (Python but Claude-only, no multi-agent layer).

`token_estimator.py` in this repo is the **reference oracle**: a working Python prototype of the parsing + attribution. The Rust command is validated against it, then `token_estimator.py` is retired.

## Why ccusage's loaders aren't enough as-is

ccusage's Claude loader is perf-tuned to **skip every line without a `"usage":{` marker** and deserialize only token fields (`adapter/claude/daily.rs`, `adapter/jsonl.rs` prefilter). It never parses `message.content`. But skill/subagent attribution needs:

- `Skill` tool_use blocks + the injected body (the `isMeta` user turn — has **no** `usage`, so it's prefiltered out today).
- `Agent`/`Task` tool_use blocks (subagent spawns) and their `tool_result` (return).
- Per-turn `content` to assign each assistant turn to its driving skill.

So the fork adds a **second record path** alongside the hot usage path: a content pass that reads non-usage records too. ccusage's adapter README explicitly invites new commands, so this is within design intent — but the second pass is the core net-new work.

## Port from ccusage (knowledge already extracted from src)

Fixes real correctness gaps the prototype has today:

| Item | ccusage source | Why |
|---|---|---|
| Dedup by `(message.id, requestId)` | `daily.rs:364-409` | resumed/forked sessions replay messages → double-count without it |
| Sidechain-replay dedup (parent replayed under new requestId → keep parent) | `daily.rs:380-390` | subagent logs replay parent messages |
| Recursive file discovery | walks subdirs | picks up `<session>/subagents/*.jsonl` |
| Path coverage: `$CLAUDE_CONFIG_DIR` (csv), `$XDG_CONFIG_HOME/claude`, `~/.claude` | `adapter/claude/paths.rs` | prototype hardcodes `~/.claude/projects` only |
| Structured `cache_creation` (5m+1h) | `types.rs:42-48` | newer transcripts split cache-write by TTL |
| agentProgress-wrapped usage (`data.message.message.usage`) | `daily.rs:140-161` | subagent inline usage |
| Validity filters (drop empty ids, `<synthetic>`, non-semver version) | `daily.rs:317-356` | skip junk records |
| `isSidechain` per record | `types.rs:18` | enables subagent-share dimension directly |

## Attribution Model (the core)

### Threads
- **Main thread** = the session transcript.
- **Subagent thread** = each `<session>/subagents/*.jsonl`, linked to its parent via the spawning `Agent`/`Task` tool_use. Subagent threads run their own skill stack.

### Skill stack (per thread)
Maintain a stack of active skills.

- **Push** on a `Skill` tool_use whose load succeeded (non-error `tool_result`).
- **Boundary (b)** — a stack frame stays active until popped; the whole stack pops at a `compact_boundary` (segment end).
- **Pop / nest rule (C) — runtime body-reference:** on invoking skill B, find the deepest frame A on the stack whose **captured body text references B** (by namespaced id `plugin:skill`, or by skill name). Static dependency declarations don't exist — measured: only 5% of 146 local skills use the formal `SUB-SKILL` keyword, 58% declare nothing, and there is no machine-readable field — so we detect references at runtime from the body we already capture.
  - **Found A** → pop everything above A, push B as A's child (B nests under A and all A's ancestors).
  - **Not found** (no active skill references B) → previous flow ended; pop the whole thread stack, push B as a new root.
- Empty stack → turns are owned by the **baseline** ("(no skill)").

### Exclusive vs inclusive
- **Exclusive(S)** — sum of attributable tokens over turns where S is the **leaf** (top of stack). A true partition: `Σ exclusive(all skills + baseline) ≈ billed`. Answers "what's driving spend right now."
- **Inclusive(S)** — `exclusive(S)` + `Σ inclusive(children of S)`, where children are skills nested under S **and subagent-thread roots spawned while S was leaf**. Subtree total; intentionally exceeds billed (nesting double-counts). Answers "what did invoking S ultimately cost." This is the "counts for both" column.

### Per-turn token attribution
For each assistant turn `t` with billed `usage = (in_t, out_t, cc_t, cr_t)`, owned by leaf `S`:

- **`out_t`, `cc_t`, `in_t`** → attributed to `S` directly. These are the *fresh* tokens the turn produced: the model's output, the new content written to cache (this turn's tool_results + assistant message), and the uncached remainder. Tool calls the skill makes and their results are counted here (they enter as `cc` on the next turn). **This is the fix for "body-only is too stupid" — the induced work is counted.**
- **`cr_t` (cache_read, the inherited prefix re-read)** → **apportioned**, not attributed gross. The re-read prefix is content added by *earlier* turns. Split `cr_t` across prior content proportional to the `cc` each skill injected:
  - Track `added[S]` = cumulative `cc` attributed to S so far on this thread.
  - `prefix_t ≈ Σ_{j<t} cc_j` (accumulated cacheable prefix).
  - `attributed_cr[S] += cr_t * added[S] / prefix_t`.
  - Shares sum to 1, so `Σ attributed_cr = Σ cr_t` — reconciles to the bill exactly. Early long-lived skills (e.g. `brainstorming`) accrue large apportioned `cr` because their early writes are re-read by every later turn — matching reality.

**Exclusive(S) = out_S + cc_S + in_S + attributed_cr_S.** Cost in $ via ccusage pricing on each component (cache_read priced at the read rate, cache_creation at the write rate, etc. — reuse ccusage `cost.rs`).

Subagent threads apportion `cr` within their own thread; the thread total then rolls up inclusively to the spawning skill and its ancestors.

This is an approximation (ignores cache eviction nuance and partial-prefix reads) but is billed-reconciling and is the principled generalization of the prototype's body-only rent. Flag it as a model, not ground truth — same honesty contract as the prototype.

## Other dimensions (the `/usage` characteristics)

Same parsed data, simpler aggregations. All windowable (`--since/--until`, default last 24h to match `/usage`):

- **High-context share** — % of token volume in requests where `in+cc+cr > 150k`.
- **Subagent share** — token-weighted `isSidechain` (or subagent-thread) share of total.
- **Long-session share** — % of usage in sessions whose span (`last−first timestamp`) ≥ N hours.
- **Per-plugin** — group skills/subagents by namespace prefix (`superpowers:`, `air-skills:`).

## Reconciliation invariants (tests assert these)

1. `Σ exclusive(skills) + exclusive(baseline) ≈ billed total` (per segment and overall).
2. `Σ attributed_cr ≈ Σ billed cache_read`.
3. `inclusive(S) ≥ exclusive(S)` for all S.
4. Pipeline never crashes on the local corpus; output round-trips JSON.

## Validation against the oracle

Port incrementally; at each step assert the Rust command's per-skill exclusive numbers match `token_estimator.py` on the same transcripts within tolerance, then extend the oracle to the new model (apportioned cr, nesting, subagents) so both stay in lock-step until retirement.

## Testing strategy

- **Parsing** — fixtures for: skill load (tool_use → stub → isMeta body), failed load, `compact_boundary`, agentProgress-wrapped usage, structured cache_creation, dedup pairs, subagent file linkage. Falsifiable, full coverage.
- **Attribution** — hand-computed fixtures for exclusive partition, apportioned cr (shares sum to 1), (C) nest-vs-sibling via body reference, subagent roll-up.
- **Corpus** — opt-in, no network, asserts invariants 1-4 on the local transcript corpus; calibration report-only.

## Out of scope (v1)

- Non-Claude adapters (Codex/Gemini/etc.) — the model is adapter-agnostic by design, but only the Claude content pass ships first.
- Cost/daily/session reporting — stock ccusage already does this; don't rebuild.

## Open risks

- **cr apportionment accuracy** — the `prefix_t ≈ Σ cc_j` approximation ignores eviction and 1h-vs-5m cache mixing. Acceptable for v1 (reconciles to bill); revisit if it skews long sessions.
- **(C) reference detection** — body-text matching of skill names has false positives (a skill mentioning another in prose without invoking it). Mitigate by only nesting when the referenced skill is *actually invoked later* in the same segment.
- **Upstream appetite** — a Claude-content-analytics command may be out of ccusage's cost-focused scope. Open a discussion first; maintain a fork if cold.
