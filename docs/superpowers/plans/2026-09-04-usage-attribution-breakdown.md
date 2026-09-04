# Usage Attribution Breakdown (Plugin / Skill / Source-Type) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add three new opt-in breakdown dimensions to ccusage reports — by plugin, by skill, and
by source-type (active/background) — driven by three new CLI flags (`--by-plugin`, `--by-skill`,
`--by-source-type`), following the exact pattern the existing `--breakdown` (by-model) flag uses.

**Architecture:** `UsageEntry` gains two new optional fields (`attribution_plugin`,
`attribution_skill`) that ride through the existing `LoadedEntry.data` wrapper with zero extra
plumbing on the claude adapter's whole-file path, and through two new fields on the claude
adapter's separate streaming (`daily.rs`) entry structs. Three new breakdown structs
(`PluginBreakdown`, `SkillBreakdown`, `SourceTypeBreakdown`) mirror `ModelBreakdown` exactly and
are accumulated **unconditionally** in `UsageAccumulator`/`DailyAccumulator`, exactly like
`ModelBreakdown` already is today (accumulation is not gated behind `--breakdown`, so the new
dimensions follow the same precedent — see Global Constraints). JSON output always includes the
three new arrays unconditionally, matching how `modelBreakdowns` is always present in JSON
regardless of `--breakdown`. Only **table rendering** is gated behind the three new flags, exactly
like `--breakdown` gates `push_breakdown_rows` today. This keeps every change contained to the
files the design spec lists — no other adapter's `report.rs` needs to change, because the shared
table renderer (`print_usage_table_with_options`) and the shared JSON builders
(`summary_json`/`session_summary_json`/`agent_summary_json`) are the single choke points every
adapter already funnels through.

**Tech Stack:** Rust (serde, insta snapshot testing, cargo test), Rust workspace under `rust/`.

**Spec:** `docs/superpowers/specs/2026-09-04-usage-attribution-breakdown-design.md`

## Global Constraints

- Flag names: `--by-plugin`, `--by-skill`, `--by-source-type` (no short aliases, matching how most
  non-`-b` shared flags are spelled).
- Field names: `attribution_plugin` / `attribution_skill` on `UsageEntry`, serde-mapped from
  `attributionPlugin` / `attributionSkill` (struct already has `#[serde(rename_all = "camelCase")]`).
- Bucket names: unknown plugin/skill → `"unattributed"`; `is_sidechain == Some(true)` →
  `"background"`, everything else → `"active"`.
- `PluginBreakdown` / `SkillBreakdown` / `SourceTypeBreakdown` mirror `ModelBreakdown`
  (`rust/crates/ccusage-core/src/types.rs:98-111`) field-for-field: name field, `input_tokens`,
  `output_tokens`, `cache_creation_tokens`, `cache_read_tokens`, `extra_total_tokens` (skip
  serializing), `cost`, `missing_pricing` (skip serializing).
- **Deviation from a strict literal reading of the design spec's "gated on flags" language**: the
  spec's Data Flow section says the three new accumulators should "only run when its flag is set,
  to avoid the extra pass when unused." The existing `ModelBreakdown` accumulation that this
  design explicitly mirrors is **not** gated behind `--breakdown` today — it always runs
  unconditionally in `UsageAccumulator::add_entry`/`DailyAccumulator::add_entry`, and
  `--breakdown` only gates whether the breakdown rows get *rendered* in the table. Threading a
  gating flag into accumulation would require changing `summarize_by_key`'s signature, which is
  called from every single adapter's `report.rs` (17+ call sites) — far outside the design spec's
  "Required Changes" file list. This plan accumulates the three new dimensions unconditionally
  (same cost class as the existing model-breakdown accumulation: a few extra hashmap operations
  per entry) and gates only table rendering behind the three new flags, and leaves JSON output
  unconditional (matching `modelBreakdowns`, which is never gated behind `--breakdown` in JSON
  either). This keeps the change contained to exactly the files the spec's "Required Changes"
  section lists.
- Out of scope (per spec): changing `--breakdown`/`--by-agent` behavior, touching the
  sidechain-replay dedup comparison logic itself, capturing new fields from non-claude adapters,
  a combined/nested single-table view across dimensions.
- This is new public CLI surface (flags + JSON schema). Per `CONTRIBUTING.md`, a GitHub issue
  should get maintainer sign-off in `ryoppippi/ccusage` before a PR opens — flag this to the user
  before Task 9's docs land, but do not block implementation on it.

---

### Task 1: Core types — `UsageEntry` fields, breakdown structs, `UsageSummary` fields

**Files:**
- Modify: `rust/crates/ccusage-core/src/types.rs`
- Modify: `rust/crates/ccusage-core/src/cost.rs:560` (fixture compile fix)
- Modify: `rust/crates/ccusage-core/src/summary.rs:691-704` (test fixture compile fix — the
  `loaded_entry()` helper in the test module)
- Modify: `rust/crates/ccusage/src/main.rs:757,842,904` (fixture compile fixes)
- Test: `rust/crates/ccusage-core/src/types.rs` (new `#[cfg(test)]` module additions)

**Interfaces:**
- Produces: `UsageEntry.attribution_plugin: Option<String>`,
  `UsageEntry.attribution_skill: Option<String>` — consumed directly via `entry.data.*` by every
  later task without any `LoadedEntry` change (LoadedEntry already wraps `data: UsageEntry`).
- Produces: `PluginBreakdown { plugin_name: String, input_tokens: u64, output_tokens: u64,
  cache_creation_tokens: u64, cache_read_tokens: u64, extra_total_tokens: u64, cost: f64,
  missing_pricing: bool }`, `SkillBreakdown { skill_name: String, ... same shape ... }`,
  `SourceTypeBreakdown { source_type: String, ... same shape ... }` — consumed by Task 3
  (daily.rs), Task 4 (summary.rs), Task 6 (output.rs), Task 7 (adapter-all/types.rs).
- Produces: `UsageSummary.plugin_breakdowns: Vec<PluginBreakdown>`,
  `UsageSummary.skill_breakdowns: Vec<SkillBreakdown>`,
  `UsageSummary.source_type_breakdowns: Vec<SourceTypeBreakdown>` — consumed by Task 4, Task 6,
  Task 7.

- [ ] **Step 1: Write the failing test for `UsageEntry` JSONL deserialization**

Add to the `#[cfg(test)] mod tests` block at the bottom of
`rust/crates/ccusage-core/src/types.rs` (after the existing `saturates_token_count_accumulation_and_total` test):

```rust
    #[test]
    fn deserializes_attribution_plugin_and_skill_from_camel_case_json() {
        let json = r#"{
            "timestamp": "2026-01-02T00:00:00.000Z",
            "message": {
                "usage": {"input_tokens": 10, "output_tokens": 5},
                "model": "claude-sonnet-4-20250514",
                "id": "msg-1"
            },
            "attributionPlugin": "aws",
            "attributionSkill": "superpowers:brainstorming",
            "isSidechain": true
        }"#;

        let entry: UsageEntry = serde_json::from_str(json).unwrap();

        assert_eq!(entry.attribution_plugin.as_deref(), Some("aws"));
        assert_eq!(
            entry.attribution_skill.as_deref(),
            Some("superpowers:brainstorming")
        );
        assert_eq!(entry.is_sidechain, Some(true));
    }

    #[test]
    fn attribution_fields_default_to_none_when_absent() {
        let json = r#"{
            "timestamp": "2026-01-02T00:00:00.000Z",
            "message": {
                "usage": {"input_tokens": 10, "output_tokens": 5},
                "model": "gpt-5.2-codex",
                "id": "msg-2"
            }
        }"#;

        let entry: UsageEntry = serde_json::from_str(json).unwrap();

        assert_eq!(entry.attribution_plugin, None);
        assert_eq!(entry.attribution_skill, None);
    }
```

Note: `serde_json` is already a dependency of `ccusage-core` (used throughout `types.rs` tests
indirectly via other modules); if this specific file's test module doesn't already have
`use serde_json` in scope, the fully-qualified call `serde_json::from_str` needs no import since
it's an external crate path.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ccusage-core deserializes_attribution_plugin_and_skill_from_camel_case_json`
Expected: FAIL with "no field `attribution_plugin` on type `UsageEntry`" (compile error).

- [ ] **Step 3: Add the two new fields to `UsageEntry`**

In `rust/crates/ccusage-core/src/types.rs`, edit the `UsageEntry` struct (lines 7-19):

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageEntry {
    pub session_id: Option<String>,
    pub timestamp: String,
    pub version: Option<String>,
    pub message: UsageMessage,
    #[serde(rename = "costUSD")]
    pub cost_usd: Option<f64>,
    pub request_id: Option<String>,
    pub is_api_error_message: Option<bool>,
    pub is_sidechain: Option<bool>,
    pub attribution_plugin: Option<String>,
    pub attribution_skill: Option<String>,
}
```

- [ ] **Step 4: Run test to verify Steps 1-2's tests pass**

Run: `cargo test -p ccusage-core deserializes_attribution_plugin_and_skill_from_camel_case_json attribution_fields_default_to_none_when_absent`
Expected: Both PASS (this also confirms the struct compiles, but downstream crates that construct
`UsageEntry` by literal will still fail to compile — fixed in Steps 5-8).

- [ ] **Step 5: Fix the `UsageEntry` literal in `cost.rs`**

Read `rust/crates/ccusage-core/src/cost.rs` around line 560 to see the exact fixture (a test
helper constructing `UsageEntry { ... is_sidechain: None, ... }` or similar). Add
`attribution_plugin: None, attribution_skill: None,` immediately after the `is_sidechain: ...`
field in that literal.

- [ ] **Step 6: Fix the `UsageEntry` literal in `summary.rs`'s test fixture**

In `rust/crates/ccusage-core/src/summary.rs`, edit the `loaded_entry()` helper (around line
691-704) inside `#[cfg(test)] mod tests`:

```rust
            data: UsageEntry {
                session_id: Some(fixture.session_id.to_string()),
                timestamp: format_rfc3339_millis(timestamp),
                version: fixture.version.map(str::to_string),
                message: UsageMessage {
                    usage,
                    model: fixture.model.map(str::to_string),
                    id: Some(format!("msg-{}", fixture.timestamp)),
                },
                cost_usd: None,
                request_id: None,
                is_api_error_message: None,
                is_sidechain: None,
                attribution_plugin: None,
                attribution_skill: None,
            },
```

- [ ] **Step 7: Fix the three `UsageEntry` literals in `rust/crates/ccusage/src/main.rs`**

Read `rust/crates/ccusage/src/main.rs` around lines 757, 842, and 904 — each is a
`data: UsageEntry { ... is_sidechain: None, ... }` test fixture literal. Add
`attribution_plugin: None, attribution_skill: None,` right after `is_sidechain: None,` in all
three.

- [ ] **Step 8: Run the whole workspace build to confirm no other `UsageEntry` literal broke**

Run: `cargo build --workspace 2>&1 | grep -A3 "missing field"`
Expected: no output. If any other `data: UsageEntry { ... }` or bare `UsageEntry { ... }` literal
surfaces (outside adapter-local types that merely share the name, e.g. each non-claude adapter has
its own unrelated local `UsageEntry`-named struct — only literals whose type resolves to
`ccusage_core::UsageEntry` matter), fix it the same way: add
`attribution_plugin: None, attribution_skill: None,`.

- [ ] **Step 9: Add the three new breakdown structs**

In `rust/crates/ccusage-core/src/types.rs`, immediately after the `ModelBreakdown` struct
definition (after line 111, before `LoadedEntry`):

```rust
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginBreakdown {
    pub plugin_name: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    #[serde(skip_serializing)]
    pub extra_total_tokens: u64,
    pub cost: f64,
    #[serde(skip_serializing)]
    pub missing_pricing: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillBreakdown {
    pub skill_name: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    #[serde(skip_serializing)]
    pub extra_total_tokens: u64,
    pub cost: f64,
    #[serde(skip_serializing)]
    pub missing_pricing: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceTypeBreakdown {
    pub source_type: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    #[serde(skip_serializing)]
    pub extra_total_tokens: u64,
    pub cost: f64,
    #[serde(skip_serializing)]
    pub missing_pricing: bool,
}
```

- [ ] **Step 10: Add the three new `Vec` fields to `UsageSummary`**

In `rust/crates/ccusage-core/src/types.rs`, edit `UsageSummary` (around line 136-169) to add the
new fields right after `model_breakdowns`:

```rust
    pub models_used: Vec<String>,
    pub model_breakdowns: Vec<ModelBreakdown>,
    pub plugin_breakdowns: Vec<PluginBreakdown>,
    pub skill_breakdowns: Vec<SkillBreakdown>,
    pub source_type_breakdowns: Vec<SourceTypeBreakdown>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
```

- [ ] **Step 11: Fix every `UsageSummary { ... }` literal across the workspace**

Run: `cargo build --workspace 2>&1 | grep -B2 "missing field.*plugin_breakdowns\|missing field.*skill_breakdowns\|missing field.*source_type_breakdowns"`

For every reported literal (this includes `rust/crates/ccusage-core/src/summary.rs`'s
`into_summary` methods and `aggregate_summaries`/test fixtures, `rust/crates/ccusage-core/src/output.rs`'s
test fixtures, and `rust/adapters/claude/src/daily.rs`'s `into_summary`), add:

```rust
            plugin_breakdowns: Vec::new(),
            skill_breakdowns: Vec::new(),
            source_type_breakdowns: Vec::new(),
```

right after the `model_breakdowns: ...` field in each literal. (Tasks 3 and 4 below replace the
`Vec::new()` placeholders in `summary.rs`'s and `daily.rs`'s *production* accumulator
`into_summary` methods with the real accumulated vectors — this step only needs to make the
workspace compile now; test-only fixtures keep the empty vecs since they don't exercise the new
dimensions.)

- [ ] **Step 12: Run the full workspace test suite**

Run: `cargo test --workspace`
Expected: PASS. Some `insta` snapshot tests that serialize `UsageSummary` directly (e.g. in
`summary.rs`, `output.rs`) will now include the three new empty-array fields
(`"pluginBreakdowns":[]` etc.) in their snapshot output and will fail with a snapshot mismatch —
this is expected. Run `cargo insta review` (or `cargo insta accept` if confident) to accept the
updated snapshots for this task's changes only; do not blanket-accept — inspect each diff to
confirm the only change is the three new empty arrays appearing.

- [ ] **Step 13: Commit**

```bash
git add rust/crates/ccusage-core/src/types.rs rust/crates/ccusage-core/src/cost.rs \
  rust/crates/ccusage-core/src/summary.rs rust/crates/ccusage/src/main.rs \
  rust/crates/ccusage-core/src/snapshots
git commit -m "feat(core): add attribution_plugin/attribution_skill fields and breakdown structs"
```

---

### Task 2: Claude adapter whole-file path (`lib.rs`) — verify propagation, add tests

**Files:**
- Modify: `rust/adapters/claude/src/lib.rs` (test module only — no production code changes; see
  rationale below)
- Test: `rust/adapters/claude/src/lib.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `UsageEntry.attribution_plugin`, `UsageEntry.attribution_skill` from Task 1.
- Produces: confidence that `LoadedEntry.data.attribution_plugin` /
  `LoadedEntry.data.attribution_skill` are populated correctly through
  `serde_json::from_slice::<UsageEntry>(line)` (line 318) and survive the sidechain-replay dedup
  in `push_deduped_entry` (lines 142-220ish), since `LoadedEntry { data: data, ... }` (around line
  348-365) moves the whole deserialized `UsageEntry` — no field-by-field copy exists to miss the
  two new fields.

**Rationale for no production code change:** `rust/adapters/claude/src/lib.rs` deserializes each
JSONL line directly into `ccusage_core::UsageEntry` (`serde_json::from_slice::<UsageEntry>(line)`
at line 318) and stores the entire struct as `LoadedEntry.data` (`data: data` — a full move, not a
field-by-field reconstruction). Because Task 1 added the two new fields to `UsageEntry` with the
struct's existing `#[serde(rename_all = "camelCase")]`, they are captured automatically. This
task only needs tests proving it.

- [ ] **Step 1: Write a failing round-trip test using the real file-reading path**

Add to `rust/adapters/claude/src/lib.rs`'s `#[cfg(test)] mod tests` block, near the existing
`read_usage_file`-based tests (around line 735):

```rust
    #[test]
    fn round_trips_attribution_plugin_and_skill_through_real_file_read() {
        let fixture = fs_fixture!(
            "projects/project-a/session-a/chat.jsonl" => [
                r#"{"timestamp":"2026-05-22T02:34:40.000Z","message":{"id":"ocgo","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":1}},"attributionPlugin":"aws","attributionSkill":"superpowers:brainstorming"}"#,
            ]
        );
        let path = fixture.path("projects/project-a/session-a/chat.jsonl");

        let loaded = read_usage_file(&path, None, CostMode::Display, None);

        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(
            loaded.entries[0].data.attribution_plugin.as_deref(),
            Some("aws")
        );
        assert_eq!(
            loaded.entries[0].data.attribution_skill.as_deref(),
            Some("superpowers:brainstorming")
        );
    }
```

(Match the exact `fs_fixture!` macro invocation style already used by the neighboring test at
line ~735 — read that test first to copy its precise macro syntax, since this plan's snippet
above approximates the call shape but the macro's argument syntax must match verbatim what's
already in the file.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ccusage-claude round_trips_attribution_plugin_and_skill_through_real_file_read`
Expected: FAIL — either a compile error if the macro call shape doesn't match, or (if it compiles)
this specific test should actually PASS already given Task 1's change, since no production code
gap exists. If it passes immediately, that confirms the "no code change needed" rationale; keep
the test as a regression guard and proceed.

- [ ] **Step 3: Extend the dedup fixture struct with attribution fields**

Edit `UsageEntryFixture` (around line 920) in `rust/adapters/claude/src/lib.rs`'s test module:

```rust
    struct UsageEntryFixture {
        message_id: &'static str,
        request_id: &'static str,
        is_sidechain: bool,
        cache_read_tokens: u64,
        output_tokens: u64,
        plugin: Option<&'static str>,
        skill: Option<&'static str>,
    }
```

- [ ] **Step 4: Thread the new fixture fields into `loaded_usage_entry()`**

Edit `loaded_usage_entry()` (around line 928-950):

```rust
    fn loaded_usage_entry(fixture: UsageEntryFixture) -> LoadedEntry {
        LoadedEntry {
            data: UsageEntry {
                session_id: Some("session-a".to_string()),
                timestamp: "2026-03-29T07:00:00.000Z".to_string(),
                version: Some("1.0.0".to_string()),
                message: UsageMessage {
                    usage: TokenUsageRaw {
                        input_tokens: 0,
                        output_tokens: fixture.output_tokens,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: fixture.cache_read_tokens,
                        speed: None,
                        cache_creation: None,
                    },
                    model: Some("claude-sonnet-4-20250514".to_string()),
                    id: Some(fixture.message_id.to_string()),
                },
                cost_usd: None,
                request_id: Some(fixture.request_id.to_string()),
                is_api_error_message: None,
                is_sidechain: Some(fixture.is_sidechain),
                attribution_plugin: fixture.plugin.map(str::to_string),
                attribution_skill: fixture.skill.map(str::to_string),
            },
            // ...rest of LoadedEntry fields unchanged from the existing implementation below this point...
```

Read the rest of the existing `loaded_usage_entry` body (lines following `is_sidechain` in the
`data:` block) and leave every other field untouched.

- [ ] **Step 5: Fix all existing `UsageEntryFixture { ... }` call sites to add the two new fields**

Run: `grep -n "UsageEntryFixture {" rust/adapters/claude/src/lib.rs`

For every call site found (the existing dedup tests, e.g.
`keeps_parent_usage_when_sidechain_replays_message_with_new_request_id` around line 827 and
`refreshes_dedupe_indexes_when_parent_replaces_sidechain_replay` around line 877), add
`plugin: None, skill: None,` to each literal so they keep compiling and keep testing the
attribution-agnostic dedup behavior unchanged.

- [ ] **Step 6: Write a dedup-safety test proving attribution fields ride along with the winning entry**

Add to the same test module:

```rust
    #[test]
    fn sidechain_replay_dedup_keeps_attribution_fields_of_winning_entry() {
        let mut deduped_indexes = Default::default();
        let mut deduped = Vec::new();

        push_deduped_entry(
            loaded_usage_entry(UsageEntryFixture {
                message_id: "msg-parent",
                request_id: "req-sidechain",
                is_sidechain: true,
                cache_read_tokens: 5,
                output_tokens: 5,
                plugin: Some("aws"),
                skill: Some("superpowers:brainstorming"),
            }),
            &mut deduped_indexes,
            &mut deduped,
        );
        push_deduped_entry(
            loaded_usage_entry(UsageEntryFixture {
                message_id: "msg-parent",
                request_id: "req-parent",
                is_sidechain: false,
                cache_read_tokens: 20,
                output_tokens: 5,
                plugin: Some("atlassian"),
                skill: None,
            }),
            &mut deduped_indexes,
            &mut deduped,
        );

        assert_eq!(deduped.len(), 1);
        assert_eq!(
            deduped[0].data.attribution_plugin.as_deref(),
            Some("atlassian")
        );
        assert_eq!(deduped[0].data.attribution_skill, None);
        assert_eq!(deduped[0].data.message.usage.cache_read_input_tokens, 20);
    }
```

This mirrors the existing `keeps_parent_usage_when_sidechain_replays_message_with_new_request_id`
test's shape and asserts the parent (non-sidechain) entry wins per the existing
`should_replace_deduped_entry` logic (untouched), and its attribution fields — not the sidechain
replay's — are what land in the deduped output. This proves no double-counting and no accidental
field mixing.

- [ ] **Step 7: Run all new and existing tests in this file**

Run: `cargo test -p ccusage-claude --lib`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add rust/adapters/claude/src/lib.rs
git commit -m "test(claude): verify attribution fields propagate through whole-file load and dedup"
```

---

### Task 3: Claude adapter streaming path (`daily.rs`) — thread fields + accumulate breakdowns

**Files:**
- Modify: `rust/adapters/claude/src/daily.rs`
- Test: `rust/adapters/claude/src/daily.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `PluginBreakdown`, `SkillBreakdown`, `SourceTypeBreakdown` from Task 1 (via
  `use crate::{...}` glob re-export from `ccusage_core`).
- Produces: `DailyLoadedEntry.plugin: Option<String>`, `DailyLoadedEntry.skill: Option<String>`
  (consumed by this task's own `DailyAccumulator`); `UsageSummary.plugin_breakdowns` /
  `skill_breakdowns` / `source_type_breakdowns` populated for every `daily` command report that
  uses this fast path — consumed by Task 6 (rendering).

- [ ] **Step 1: Add the two new fields to `DailyUsageEntry` and `DailyLoadedEntry`**

In `rust/adapters/claude/src/daily.rs`, edit `DailyLoadedEntry` (lines 114-127):

```rust
#[derive(Debug)]
struct DailyLoadedEntry {
    timestamp: TimestampMs,
    date: String,
    project: Arc<str>,
    session_id: Arc<str>,
    usage: TokenUsageRaw,
    cost: f64,
    model: Option<String>,
    missing_pricing_model: Option<String>,
    message_id: Option<String>,
    request_id: Option<String>,
    is_sidechain: Option<bool>,
    plugin: Option<String>,
    skill: Option<String>,
}
```

Edit `DailyUsageEntry` (lines 129-140):

```rust
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DailyUsageEntry {
    timestamp: String,
    message: DailyUsageMessage,
    version: Option<String>,
    session_id: Option<String>,
    #[serde(rename = "costUSD")]
    cost_usd: Option<f64>,
    request_id: Option<String>,
    is_sidechain: Option<bool>,
    attribution_plugin: Option<String>,
    attribution_skill: Option<String>,
}
```

- [ ] **Step 2: Add the two new fields to `DailyAgentProgressMessage` and thread through `into_entry`**

Edit `DailyAgentProgressMessage` (lines 178-187):

```rust
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DailyAgentProgressMessage {
    timestamp: String,
    message: DailyUsageMessage,
    #[serde(rename = "costUSD")]
    cost_usd: Option<f64>,
    request_id: Option<String>,
    is_sidechain: Option<bool>,
    attribution_plugin: Option<String>,
    attribution_skill: Option<String>,
}
```

Edit `DailyUsageLine::into_entry` (lines 149-164):

```rust
impl DailyUsageLine {
    fn into_entry(self) -> DailyUsageEntry {
        match self {
            DailyUsageLine::Direct(entry) => entry,
            DailyUsageLine::AgentProgress(entry) => DailyUsageEntry {
                timestamp: entry.data.message.timestamp,
                message: entry.data.message.message,
                version: None,
                session_id: entry.session_id,
                cost_usd: entry.data.message.cost_usd,
                request_id: entry.data.message.request_id,
                is_sidechain: entry.data.message.is_sidechain,
                attribution_plugin: entry.data.message.attribution_plugin,
                attribution_skill: entry.data.message.attribution_skill,
            },
        }
    }
}
```

- [ ] **Step 3: Thread the fields into both `DailyLoadedEntry` construction sites**

In `rust/adapters/claude/src/daily.rs`, find the primary entry construction (around line
318-330) and edit it to clone the attribution fields before `data.message.id` /
`data.request_id` get moved into the struct literal:

```rust
        loaded_file.entries.push(DailyLoadedEntry {
            timestamp,
            date,
            project: Arc::clone(&project),
            session_id: Arc::clone(&session_id),
            usage: data.message.usage,
            cost,
            model: data.message.model.clone(),
            missing_pricing_model,
            message_id: data.message.id,
            request_id: data.request_id,
            is_sidechain: data.is_sidechain,
            plugin: data.attribution_plugin.clone(),
            skill: data.attribution_skill.clone(),
        });
```

(Read the exact surrounding code first — the field list above reflects the fields already known
from the struct definition; keep every other field/line exactly as it exists today, only add the
two new `plugin`/`skill` lines before the closing brace, positioned after `is_sidechain`.)

Find the second construction site inside the "advisor" entries loop (around line 339-360) and add
the same two lines, cloning again since this site is inside a loop that may run more than once:

```rust
                loaded_file.entries.push(DailyLoadedEntry {
                    // ...existing fields for the advisor sub-entry, unchanged...
                    is_sidechain: data.is_sidechain,
                    plugin: data.attribution_plugin.clone(),
                    skill: data.attribution_skill.clone(),
                });
```

- [ ] **Step 4: Fix the `DailyUsageEntry` literals used in `DailyUsageLine::into_entry`'s `Direct` variant path and any raw-construction test fixtures**

Run: `cargo build -p ccusage-claude 2>&1 | grep -A3 "missing field"`

Fix every reported `DailyUsageEntry { ... }` or `DailyAgentProgressMessage { ... }` literal by
adding `attribution_plugin: None, attribution_skill: None,` (or wiring real values if the literal
is a JSON-fixture-driven test — handled in Step 8 below).

- [ ] **Step 5: Add plugin/skill accumulation fields to `DailyAccumulator`**

Edit the `use crate::{...}` import block near the top of `daily.rs` (around line 13-16) to add
the three breakdown types:

```rust
use crate::{
    ModelBreakdown, PluginBreakdown, PricingMap, Result, SkillBreakdown, SourceTypeBreakdown,
    Speed, TimestampMs, TokenCounts, TokenUsageRaw,
```

(Keep every other name already imported on that line/block exactly as-is — only insert the three
new names alphabetically alongside the existing ones.)

Edit the `DailyAccumulator` struct (around line 517-524):

```rust
#[derive(Default)]
struct DailyAccumulator {
    counts: TokenCounts,
    cost: f64,
    models: Vec<String>,
    breakdowns: Vec<ModelBreakdown>,
    breakdown_indexes: FxHashMap<String, usize>,
    plugin_breakdowns: Vec<PluginBreakdown>,
    plugin_breakdown_indexes: FxHashMap<String, usize>,
    skill_breakdowns: Vec<SkillBreakdown>,
    skill_breakdown_indexes: FxHashMap<String, usize>,
    source_type_breakdowns: Vec<SourceTypeBreakdown>,
    source_type_breakdown_indexes: FxHashMap<String, usize>,
}
```

- [ ] **Step 6: Accumulate the three new dimensions in `DailyAccumulator::add_entry`**

Edit `add_entry` (around line 527-554) to add three new blocks after the existing `if let
Some(model) = &entry.model { ... }` block, before the closing brace of the method:

```rust
    fn add_entry(&mut self, entry: &DailyLoadedEntry) {
        self.counts.add_usage(entry.usage);
        self.cost += entry.cost;
        if let Some(model) = &entry.model {
            let model = crate::model_aliases::resolve_model_name(model).into_owned();
            let index = if let Some(index) = self.breakdown_indexes.get(model.as_str()) {
                *index
            } else {
                let index = self.breakdowns.len();
                self.breakdown_indexes.insert(model.clone(), index);
                self.models.push(model.clone());
                self.breakdowns.push(ModelBreakdown {
                    model_name: model.clone(),
                    ..ModelBreakdown::default()
                });
                index
            };
            let breakdown = &mut self.breakdowns[index];
            breakdown.input_tokens += entry.usage.input_tokens;
            breakdown.output_tokens += entry.usage.output_tokens;
            breakdown.cache_creation_tokens += entry.usage.cache_creation_token_count();
            breakdown.cache_read_tokens += entry.usage.cache_read_input_tokens;
            breakdown.cost += entry.cost;
            if entry.missing_pricing_model.is_some() {
                breakdown.missing_pricing = true;
            }
        }

        let plugin_key = entry.plugin.as_deref().unwrap_or("unattributed");
        let plugin_index = if let Some(index) = self.plugin_breakdown_indexes.get(plugin_key) {
            *index
        } else {
            let index = self.plugin_breakdowns.len();
            self.plugin_breakdown_indexes
                .insert(plugin_key.to_string(), index);
            self.plugin_breakdowns.push(PluginBreakdown {
                plugin_name: plugin_key.to_string(),
                ..PluginBreakdown::default()
            });
            index
        };
        let plugin_breakdown = &mut self.plugin_breakdowns[plugin_index];
        plugin_breakdown.input_tokens += entry.usage.input_tokens;
        plugin_breakdown.output_tokens += entry.usage.output_tokens;
        plugin_breakdown.cache_creation_tokens += entry.usage.cache_creation_token_count();
        plugin_breakdown.cache_read_tokens += entry.usage.cache_read_input_tokens;
        plugin_breakdown.cost += entry.cost;
        if entry.missing_pricing_model.is_some() {
            plugin_breakdown.missing_pricing = true;
        }

        let skill_key = entry.skill.as_deref().unwrap_or("unattributed");
        let skill_index = if let Some(index) = self.skill_breakdown_indexes.get(skill_key) {
            *index
        } else {
            let index = self.skill_breakdowns.len();
            self.skill_breakdown_indexes
                .insert(skill_key.to_string(), index);
            self.skill_breakdowns.push(SkillBreakdown {
                skill_name: skill_key.to_string(),
                ..SkillBreakdown::default()
            });
            index
        };
        let skill_breakdown = &mut self.skill_breakdowns[skill_index];
        skill_breakdown.input_tokens += entry.usage.input_tokens;
        skill_breakdown.output_tokens += entry.usage.output_tokens;
        skill_breakdown.cache_creation_tokens += entry.usage.cache_creation_token_count();
        skill_breakdown.cache_read_tokens += entry.usage.cache_read_input_tokens;
        skill_breakdown.cost += entry.cost;
        if entry.missing_pricing_model.is_some() {
            skill_breakdown.missing_pricing = true;
        }

        let source_type_key = if entry.is_sidechain == Some(true) {
            "background"
        } else {
            "active"
        };
        let source_type_index =
            if let Some(index) = self.source_type_breakdown_indexes.get(source_type_key) {
                *index
            } else {
                let index = self.source_type_breakdowns.len();
                self.source_type_breakdown_indexes
                    .insert(source_type_key.to_string(), index);
                self.source_type_breakdowns.push(SourceTypeBreakdown {
                    source_type: source_type_key.to_string(),
                    ..SourceTypeBreakdown::default()
                });
                index
            };
        let source_type_breakdown = &mut self.source_type_breakdowns[source_type_index];
        source_type_breakdown.input_tokens += entry.usage.input_tokens;
        source_type_breakdown.output_tokens += entry.usage.output_tokens;
        source_type_breakdown.cache_creation_tokens += entry.usage.cache_creation_token_count();
        source_type_breakdown.cache_read_tokens += entry.usage.cache_read_input_tokens;
        source_type_breakdown.cost += entry.cost;
        if entry.missing_pricing_model.is_some() {
            source_type_breakdown.missing_pricing = true;
        }
    }
```

- [ ] **Step 7: Populate the new `UsageSummary` fields in `into_summary`**

Edit `into_summary` (around line 556-579):

```rust
    fn into_summary(mut self) -> UsageSummary {
        self.breakdowns.sort_by(|a, b| b.cost.total_cmp(&a.cost));
        self.plugin_breakdowns
            .sort_by(|a, b| b.cost.total_cmp(&a.cost));
        self.skill_breakdowns
            .sort_by(|a, b| b.cost.total_cmp(&a.cost));
        self.source_type_breakdowns
            .sort_by(|a, b| b.cost.total_cmp(&a.cost));
        UsageSummary {
            date: None,
            month: None,
            week: None,
            session_id: None,
            project_path: None,
            last_activity: None,
            first_activity: None,
            input_tokens: self.counts.input_tokens,
            output_tokens: self.counts.output_tokens,
            cache_creation_tokens: self.counts.cache_creation_tokens,
            cache_read_tokens: self.counts.cache_read_tokens,
            extra_total_tokens: 0,
            total_cost: self.cost,
            credits: None,
            message_count: None,
            models_used: self.models,
            model_breakdowns: self.breakdowns,
            plugin_breakdowns: self.plugin_breakdowns,
            skill_breakdowns: self.skill_breakdowns,
            source_type_breakdowns: self.source_type_breakdowns,
            project: None,
            versions: None,
        }
    }
```

- [ ] **Step 8: Extend the test fixture struct and helper, add a propagation test**

Edit `DailyEntryFixture` (around line 878-884):

```rust
    struct DailyEntryFixture {
        message_id: &'static str,
        request_id: &'static str,
        is_sidechain: bool,
        cache_read_tokens: u64,
        output_tokens: u64,
        plugin: Option<&'static str>,
        skill: Option<&'static str>,
    }
```

Edit `daily_loaded_entry()` (around line 886-907):

```rust
    fn daily_loaded_entry(fixture: DailyEntryFixture) -> DailyLoadedEntry {
        DailyLoadedEntry {
            timestamp: TimestampMs::from_millis(1_774_000_000_000),
            date: "2026-03-29".to_string(),
            project: Arc::from("project-a"),
            session_id: Arc::from("session-a"),
            usage: TokenUsageRaw {
                input_tokens: 0,
                output_tokens: fixture.output_tokens,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: fixture.cache_read_tokens,
                speed: None,
                cache_creation: None,
            },
            cost: 0.0,
            model: Some("claude-sonnet-4-20250514".to_string()),
            missing_pricing_model: None,
            message_id: Some(fixture.message_id.to_string()),
            request_id: Some(fixture.request_id.to_string()),
            is_sidechain: Some(fixture.is_sidechain),
            plugin: fixture.plugin.map(str::to_string),
            skill: fixture.skill.map(str::to_string),
        }
    }
```

Fix every existing `DailyEntryFixture { ... }` call site (run
`grep -n "DailyEntryFixture {" rust/adapters/claude/src/daily.rs`) to add `plugin: None, skill: None,`.

Add a new test near the existing `propagates_sidechain_metadata_from_agent_progress_lines` test
(around line 695-745), mirroring its `fs_fixture!` shape but asserting attribution fields:

```rust
    #[test]
    fn accumulates_plugin_skill_and_source_type_breakdowns() {
        let mut accumulator = DailyAccumulator::default();
        accumulator.add_entry(&daily_loaded_entry(DailyEntryFixture {
            message_id: "msg-1",
            request_id: "req-1",
            is_sidechain: false,
            cache_read_tokens: 0,
            output_tokens: 10,
            plugin: Some("aws"),
            skill: Some("superpowers:brainstorming"),
        }));
        accumulator.add_entry(&daily_loaded_entry(DailyEntryFixture {
            message_id: "msg-2",
            request_id: "req-2",
            is_sidechain: true,
            cache_read_tokens: 0,
            output_tokens: 5,
            plugin: None,
            skill: None,
        }));

        let summary = accumulator.into_summary();

        assert_eq!(summary.plugin_breakdowns.len(), 2);
        assert!(
            summary
                .plugin_breakdowns
                .iter()
                .any(|b| b.plugin_name == "aws")
        );
        assert!(
            summary
                .plugin_breakdowns
                .iter()
                .any(|b| b.plugin_name == "unattributed")
        );
        assert_eq!(summary.skill_breakdowns.len(), 2);
        assert_eq!(summary.source_type_breakdowns.len(), 2);
        assert!(
            summary
                .source_type_breakdowns
                .iter()
                .any(|b| b.source_type == "active")
        );
        assert!(
            summary
                .source_type_breakdowns
                .iter()
                .any(|b| b.source_type == "background")
        );
    }
```

- [ ] **Step 9: Run the tests**

Run: `cargo test -p ccusage-claude --lib`
Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add rust/adapters/claude/src/daily.rs
git commit -m "feat(claude): accumulate plugin/skill/source-type breakdowns in daily streaming path"
```

---

### Task 4: Core aggregation — `UsageAccumulator` and `aggregate_summaries` in `summary.rs`

**Files:**
- Modify: `rust/crates/ccusage-core/src/summary.rs`
- Test: `rust/crates/ccusage-core/src/summary.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `PluginBreakdown`, `SkillBreakdown`, `SourceTypeBreakdown` from Task 1;
  `LoadedEntry.data.attribution_plugin` / `.attribution_skill` / `.is_sidechain` (all already
  accessible, no LoadedEntry change needed).
- Produces: `UsageSummary.plugin_breakdowns` / `.skill_breakdowns` / `.source_type_breakdowns`
  populated for every non-daily-fast-path report (weekly, monthly, session, and any adapter using
  the generic `summarize_by_key`/`SessionAccumulator` machinery) — consumed by Task 6, Task 7.

- [ ] **Step 1: Import the three new breakdown types**

Edit the `use crate::{...}` block at the top of `rust/crates/ccusage-core/src/summary.rs`
(lines 6-12):

```rust
use crate::{
    LoadedEntry, ModelBreakdown, PluginBreakdown, Result, SkillBreakdown, SourceTypeBreakdown,
    TimestampMs, TokenCounts, UsageSummary,
    cli::{SharedArgs, SortOrder, WeekDay},
    cli_error,
    fast::{FxHashMap, FxHashSet},
    format_naive_date, format_rfc3339_millis, parse_iso_date,
};
```

- [ ] **Step 2: Write a failing test for `UsageAccumulator`'s new breakdowns**

Add to `#[cfg(test)] mod tests` in `summary.rs`, near `tracks_missing_pricing_in_model_breakdowns`:

```rust
    #[test]
    fn accumulates_plugin_skill_and_source_type_breakdowns_with_unattributed_and_active_defaults()
     {
        let mut accumulator = SessionAccumulator::default();
        let mut attributed = loaded_entry(LoadedEntryFixture {
            date: "2026-01-02",
            timestamp: 1_767_316_800_000,
            session_id: "session-a",
            project_path: "/workspace/project",
            model: Some("claude-sonnet-4-20250514"),
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            extra_total_tokens: 0,
            cost: 0.1,
            credits: None,
            message_count: Some(1),
            version: Some("1.0.0"),
            missing_pricing_model: None,
        });
        attributed.data.attribution_plugin = Some("aws".to_string());
        attributed.data.attribution_skill = Some("superpowers:brainstorming".to_string());
        attributed.data.is_sidechain = Some(true);
        accumulator.add_entry(&attributed);

        let mut plain = loaded_entry(LoadedEntryFixture {
            date: "2026-01-02",
            timestamp: 1_767_316_801_000,
            session_id: "session-a",
            project_path: "/workspace/project",
            model: Some("claude-sonnet-4-20250514"),
            input_tokens: 20,
            output_tokens: 10,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            extra_total_tokens: 0,
            cost: 0.02,
            credits: None,
            message_count: Some(1),
            version: Some("1.0.0"),
            missing_pricing_model: None,
        });
        plain.data.is_sidechain = Some(false);
        accumulator.add_entry(&plain);

        let row = accumulator.into_summary().unwrap();

        assert_eq!(row.plugin_breakdowns.len(), 2);
        assert!(row.plugin_breakdowns.iter().any(|b| b.plugin_name == "aws"));
        assert!(
            row.plugin_breakdowns
                .iter()
                .any(|b| b.plugin_name == "unattributed")
        );
        assert_eq!(row.skill_breakdowns.len(), 2);
        assert_eq!(row.source_type_breakdowns.len(), 2);
        assert!(
            row.source_type_breakdowns
                .iter()
                .any(|b| b.source_type == "background" && b.input_tokens == 100)
        );
        assert!(
            row.source_type_breakdowns
                .iter()
                .any(|b| b.source_type == "active" && b.input_tokens == 20)
        );
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p ccusage-core accumulates_plugin_skill_and_source_type_breakdowns_with_unattributed_and_active_defaults`
Expected: FAIL — `row.plugin_breakdowns` is empty (all zeros from Task 1's Step 11 placeholder)
since `UsageAccumulator` doesn't populate it yet.

- [ ] **Step 4: Add the six new fields to `UsageAccumulator`**

Edit the `UsageAccumulator` struct (lines 39-48):

```rust
#[derive(Default)]
struct UsageAccumulator {
    counts: TokenCounts,
    cost: f64,
    credits: Option<f64>,
    message_count: Option<u64>,
    models: Vec<String>,
    breakdowns: Vec<ModelBreakdown>,
    breakdown_indexes: FxHashMap<String, usize>,
    plugin_breakdowns: Vec<PluginBreakdown>,
    plugin_breakdown_indexes: FxHashMap<String, usize>,
    skill_breakdowns: Vec<SkillBreakdown>,
    skill_breakdown_indexes: FxHashMap<String, usize>,
    source_type_breakdowns: Vec<SourceTypeBreakdown>,
    source_type_breakdown_indexes: FxHashMap<String, usize>,
}
```

- [ ] **Step 5: Accumulate the three new dimensions in `add_entry`**

Edit `UsageAccumulator::add_entry` (lines 51-97) to add three new blocks after the existing
`if let Some(model) = &entry.model { ... }` block, before the closing brace of the method:

```rust
        let plugin_key = entry
            .data
            .attribution_plugin
            .as_deref()
            .unwrap_or("unattributed");
        let plugin_index = if let Some(index) = self.plugin_breakdown_indexes.get(plugin_key) {
            *index
        } else {
            let index = self.plugin_breakdowns.len();
            self.plugin_breakdown_indexes
                .insert(plugin_key.to_string(), index);
            self.plugin_breakdowns.push(PluginBreakdown {
                plugin_name: plugin_key.to_string(),
                ..PluginBreakdown::default()
            });
            index
        };
        let plugin_breakdown = &mut self.plugin_breakdowns[plugin_index];
        plugin_breakdown.input_tokens = plugin_breakdown
            .input_tokens
            .saturating_add(usage.input_tokens);
        plugin_breakdown.output_tokens = plugin_breakdown
            .output_tokens
            .saturating_add(usage.output_tokens);
        plugin_breakdown.cache_creation_tokens = plugin_breakdown
            .cache_creation_tokens
            .saturating_add(usage.cache_creation_token_count());
        plugin_breakdown.cache_read_tokens = plugin_breakdown
            .cache_read_tokens
            .saturating_add(usage.cache_read_input_tokens);
        plugin_breakdown.extra_total_tokens = plugin_breakdown
            .extra_total_tokens
            .saturating_add(entry.extra_total_tokens);
        plugin_breakdown.cost += entry.cost;
        if entry.missing_pricing_model.is_some() {
            plugin_breakdown.missing_pricing = true;
        }

        let skill_key = entry
            .data
            .attribution_skill
            .as_deref()
            .unwrap_or("unattributed");
        let skill_index = if let Some(index) = self.skill_breakdown_indexes.get(skill_key) {
            *index
        } else {
            let index = self.skill_breakdowns.len();
            self.skill_breakdown_indexes
                .insert(skill_key.to_string(), index);
            self.skill_breakdowns.push(SkillBreakdown {
                skill_name: skill_key.to_string(),
                ..SkillBreakdown::default()
            });
            index
        };
        let skill_breakdown = &mut self.skill_breakdowns[skill_index];
        skill_breakdown.input_tokens = skill_breakdown
            .input_tokens
            .saturating_add(usage.input_tokens);
        skill_breakdown.output_tokens = skill_breakdown
            .output_tokens
            .saturating_add(usage.output_tokens);
        skill_breakdown.cache_creation_tokens = skill_breakdown
            .cache_creation_tokens
            .saturating_add(usage.cache_creation_token_count());
        skill_breakdown.cache_read_tokens = skill_breakdown
            .cache_read_tokens
            .saturating_add(usage.cache_read_input_tokens);
        skill_breakdown.extra_total_tokens = skill_breakdown
            .extra_total_tokens
            .saturating_add(entry.extra_total_tokens);
        skill_breakdown.cost += entry.cost;
        if entry.missing_pricing_model.is_some() {
            skill_breakdown.missing_pricing = true;
        }

        let source_type_key = if entry.data.is_sidechain == Some(true) {
            "background"
        } else {
            "active"
        };
        let source_type_index =
            if let Some(index) = self.source_type_breakdown_indexes.get(source_type_key) {
                *index
            } else {
                let index = self.source_type_breakdowns.len();
                self.source_type_breakdown_indexes
                    .insert(source_type_key.to_string(), index);
                self.source_type_breakdowns.push(SourceTypeBreakdown {
                    source_type: source_type_key.to_string(),
                    ..SourceTypeBreakdown::default()
                });
                index
            };
        let source_type_breakdown = &mut self.source_type_breakdowns[source_type_index];
        source_type_breakdown.input_tokens = source_type_breakdown
            .input_tokens
            .saturating_add(usage.input_tokens);
        source_type_breakdown.output_tokens = source_type_breakdown
            .output_tokens
            .saturating_add(usage.output_tokens);
        source_type_breakdown.cache_creation_tokens = source_type_breakdown
            .cache_creation_tokens
            .saturating_add(usage.cache_creation_token_count());
        source_type_breakdown.cache_read_tokens = source_type_breakdown
            .cache_read_tokens
            .saturating_add(usage.cache_read_input_tokens);
        source_type_breakdown.extra_total_tokens = source_type_breakdown
            .extra_total_tokens
            .saturating_add(entry.extra_total_tokens);
        source_type_breakdown.cost += entry.cost;
        if entry.missing_pricing_model.is_some() {
            source_type_breakdown.missing_pricing = true;
        }
```

Note `usage` here is the same `let usage = entry.data.message.usage;` binding already at the top
of `add_entry` — no new variable needed.

- [ ] **Step 6: Populate the new `UsageSummary` fields in `into_summary`**

Edit `UsageAccumulator::into_summary` (lines 99-123):

```rust
    fn into_summary(mut self) -> UsageSummary {
        self.breakdowns.sort_by(|a, b| b.cost.total_cmp(&a.cost));
        self.plugin_breakdowns
            .sort_by(|a, b| b.cost.total_cmp(&a.cost));
        self.skill_breakdowns
            .sort_by(|a, b| b.cost.total_cmp(&a.cost));
        self.source_type_breakdowns
            .sort_by(|a, b| b.cost.total_cmp(&a.cost));
        UsageSummary {
            date: None,
            month: None,
            week: None,
            session_id: None,
            project_path: None,
            last_activity: None,
            first_activity: None,
            input_tokens: self.counts.input_tokens,
            output_tokens: self.counts.output_tokens,
            cache_creation_tokens: self.counts.cache_creation_tokens,
            cache_read_tokens: self.counts.cache_read_tokens,
            extra_total_tokens: self.counts.extra_total_tokens,
            total_cost: self.cost,
            credits: self.credits,
            message_count: self.message_count,
            models_used: self.models,
            model_breakdowns: self.breakdowns,
            plugin_breakdowns: self.plugin_breakdowns,
            skill_breakdowns: self.skill_breakdowns,
            source_type_breakdowns: self.source_type_breakdowns,
            project: None,
            versions: None,
        }
    }
```

- [ ] **Step 7: Run test to verify it passes**

Run: `cargo test -p ccusage-core accumulates_plugin_skill_and_source_type_breakdowns_with_unattributed_and_active_defaults`
Expected: PASS.

- [ ] **Step 8: Add merge logic for weekly/monthly bucket aggregation in `aggregate_summaries`**

`aggregate_summaries` (lines 208-289) rebuilds a `UsageSummary` from multiple already-summarized
rows (used by `summarize_summaries_by_bucket` for weekly/monthly reports). Edit it to merge the
three new dimensions the same way `model_breakdowns` is merged:

```rust
fn aggregate_summaries(rows: &[&UsageSummary]) -> UsageSummary {
    let mut summary = UsageSummary {
        date: None,
        month: None,
        week: None,
        session_id: None,
        project_path: None,
        last_activity: None,
        first_activity: None,
        input_tokens: 0,
        output_tokens: 0,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        extra_total_tokens: 0,
        total_cost: 0.0,
        credits: None,
        message_count: None,
        models_used: Vec::new(),
        model_breakdowns: Vec::new(),
        plugin_breakdowns: Vec::new(),
        skill_breakdowns: Vec::new(),
        source_type_breakdowns: Vec::new(),
        project: None,
        versions: None,
    };
    let mut seen_models = FxHashSet::default();
    let mut breakdown_indexes = FxHashMap::<String, usize>::default();
    let mut plugin_breakdown_indexes = FxHashMap::<String, usize>::default();
    let mut skill_breakdown_indexes = FxHashMap::<String, usize>::default();
    let mut source_type_breakdown_indexes = FxHashMap::<String, usize>::default();

    for row in rows {
        summary.input_tokens = summary.input_tokens.saturating_add(row.input_tokens);
        summary.output_tokens = summary.output_tokens.saturating_add(row.output_tokens);
        summary.cache_creation_tokens = summary
            .cache_creation_tokens
            .saturating_add(row.cache_creation_tokens);
        summary.cache_read_tokens = summary
            .cache_read_tokens
            .saturating_add(row.cache_read_tokens);
        summary.extra_total_tokens = summary
            .extra_total_tokens
            .saturating_add(row.extra_total_tokens);
        summary.total_cost += row.total_cost;
        if let Some(credits) = row.credits {
            *summary.credits.get_or_insert(0.0) += credits;
        }
        if let Some(message_count) = row.message_count {
            *summary.message_count.get_or_insert(0) += message_count;
        }
        for model in &row.models_used {
            if seen_models.insert(model.clone()) {
                summary.models_used.push(model.clone());
            }
        }
        for item in &row.model_breakdowns {
            let index = if let Some(index) = breakdown_indexes.get(item.model_name.as_str()) {
                *index
            } else {
                let index = summary.model_breakdowns.len();
                breakdown_indexes.insert(item.model_name.clone(), index);
                summary.model_breakdowns.push(ModelBreakdown {
                    model_name: item.model_name.clone(),
                    ..ModelBreakdown::default()
                });
                index
            };
            let breakdown = &mut summary.model_breakdowns[index];
            breakdown.input_tokens = breakdown.input_tokens.saturating_add(item.input_tokens);
            breakdown.output_tokens = breakdown.output_tokens.saturating_add(item.output_tokens);
            breakdown.cache_creation_tokens = breakdown
                .cache_creation_tokens
                .saturating_add(item.cache_creation_tokens);
            breakdown.cache_read_tokens = breakdown
                .cache_read_tokens
                .saturating_add(item.cache_read_tokens);
            breakdown.extra_total_tokens = breakdown
                .extra_total_tokens
                .saturating_add(item.extra_total_tokens);
            breakdown.cost += item.cost;
            breakdown.missing_pricing |= item.missing_pricing;
        }
        for item in &row.plugin_breakdowns {
            let index = if let Some(index) = plugin_breakdown_indexes.get(item.plugin_name.as_str())
            {
                *index
            } else {
                let index = summary.plugin_breakdowns.len();
                plugin_breakdown_indexes.insert(item.plugin_name.clone(), index);
                summary.plugin_breakdowns.push(PluginBreakdown {
                    plugin_name: item.plugin_name.clone(),
                    ..PluginBreakdown::default()
                });
                index
            };
            let breakdown = &mut summary.plugin_breakdowns[index];
            breakdown.input_tokens = breakdown.input_tokens.saturating_add(item.input_tokens);
            breakdown.output_tokens = breakdown.output_tokens.saturating_add(item.output_tokens);
            breakdown.cache_creation_tokens = breakdown
                .cache_creation_tokens
                .saturating_add(item.cache_creation_tokens);
            breakdown.cache_read_tokens = breakdown
                .cache_read_tokens
                .saturating_add(item.cache_read_tokens);
            breakdown.extra_total_tokens = breakdown
                .extra_total_tokens
                .saturating_add(item.extra_total_tokens);
            breakdown.cost += item.cost;
            breakdown.missing_pricing |= item.missing_pricing;
        }
        for item in &row.skill_breakdowns {
            let index = if let Some(index) = skill_breakdown_indexes.get(item.skill_name.as_str())
            {
                *index
            } else {
                let index = summary.skill_breakdowns.len();
                skill_breakdown_indexes.insert(item.skill_name.clone(), index);
                summary.skill_breakdowns.push(SkillBreakdown {
                    skill_name: item.skill_name.clone(),
                    ..SkillBreakdown::default()
                });
                index
            };
            let breakdown = &mut summary.skill_breakdowns[index];
            breakdown.input_tokens = breakdown.input_tokens.saturating_add(item.input_tokens);
            breakdown.output_tokens = breakdown.output_tokens.saturating_add(item.output_tokens);
            breakdown.cache_creation_tokens = breakdown
                .cache_creation_tokens
                .saturating_add(item.cache_creation_tokens);
            breakdown.cache_read_tokens = breakdown
                .cache_read_tokens
                .saturating_add(item.cache_read_tokens);
            breakdown.extra_total_tokens = breakdown
                .extra_total_tokens
                .saturating_add(item.extra_total_tokens);
            breakdown.cost += item.cost;
            breakdown.missing_pricing |= item.missing_pricing;
        }
        for item in &row.source_type_breakdowns {
            let index = if let Some(index) =
                source_type_breakdown_indexes.get(item.source_type.as_str())
            {
                *index
            } else {
                let index = summary.source_type_breakdowns.len();
                source_type_breakdown_indexes.insert(item.source_type.clone(), index);
                summary.source_type_breakdowns.push(SourceTypeBreakdown {
                    source_type: item.source_type.clone(),
                    ..SourceTypeBreakdown::default()
                });
                index
            };
            let breakdown = &mut summary.source_type_breakdowns[index];
            breakdown.input_tokens = breakdown.input_tokens.saturating_add(item.input_tokens);
            breakdown.output_tokens = breakdown.output_tokens.saturating_add(item.output_tokens);
            breakdown.cache_creation_tokens = breakdown
                .cache_creation_tokens
                .saturating_add(item.cache_creation_tokens);
            breakdown.cache_read_tokens = breakdown
                .cache_read_tokens
                .saturating_add(item.cache_read_tokens);
            breakdown.extra_total_tokens = breakdown
                .extra_total_tokens
                .saturating_add(item.extra_total_tokens);
            breakdown.cost += item.cost;
            breakdown.missing_pricing |= item.missing_pricing;
        }
    }
    summary
        .model_breakdowns
        .sort_by(|a, b| b.cost.total_cmp(&a.cost));
    summary
        .plugin_breakdowns
        .sort_by(|a, b| b.cost.total_cmp(&a.cost));
    summary
        .skill_breakdowns
        .sort_by(|a, b| b.cost.total_cmp(&a.cost));
    summary
        .source_type_breakdowns
        .sort_by(|a, b| b.cost.total_cmp(&a.cost));
    summary
}
```

- [ ] **Step 9: Write a test for bucket-merge behavior**

Add near `snapshots_bucket_aggregation_for_week_boundaries_invalid_dates_and_model_merging`:

```rust
    #[test]
    fn bucket_aggregation_merges_plugin_skill_and_source_type_breakdowns_across_rows() {
        let mut first = summary_row(SummaryFixture {
            date: Some("2026-01-01"),
            model: "claude-sonnet-4-20250514",
            cost: 0.1,
            input_tokens: 100,
        });
        first.plugin_breakdowns = vec![PluginBreakdown {
            plugin_name: "aws".to_string(),
            input_tokens: 100,
            cost: 0.1,
            ..PluginBreakdown::default()
        }];
        first.source_type_breakdowns = vec![SourceTypeBreakdown {
            source_type: "active".to_string(),
            input_tokens: 100,
            cost: 0.1,
            ..SourceTypeBreakdown::default()
        }];
        let mut second = summary_row(SummaryFixture {
            date: Some("2026-01-02"),
            model: "claude-sonnet-4-20250514",
            cost: 0.2,
            input_tokens: 50,
        });
        second.plugin_breakdowns = vec![PluginBreakdown {
            plugin_name: "aws".to_string(),
            input_tokens: 50,
            cost: 0.2,
            ..PluginBreakdown::default()
        }];
        second.source_type_breakdowns = vec![SourceTypeBreakdown {
            source_type: "background".to_string(),
            input_tokens: 50,
            cost: 0.2,
            ..SourceTypeBreakdown::default()
        }];

        let weekly = summarize_summaries_by_bucket(
            &[first, second],
            BucketKind::Weekly,
            WeekDay::Monday,
        );

        assert_eq!(weekly.len(), 1);
        assert_eq!(weekly[0].plugin_breakdowns.len(), 1);
        assert_eq!(weekly[0].plugin_breakdowns[0].input_tokens, 150);
        assert_eq!(weekly[0].source_type_breakdowns.len(), 2);
    }
```

- [ ] **Step 10: Run all summary.rs tests**

Run: `cargo test -p ccusage-core --lib summary::`
Expected: PASS.

- [ ] **Step 11: Run the full workspace test suite and review insta snapshots**

Run: `cargo test --workspace`

Expected: any snapshot test that serializes a full `UsageSummary` (e.g.
`snapshots_summarize_by_key_aggregates_counts_costs_and_breakdowns`,
`snapshots_session_accumulator_latest_metadata_versions_and_timezone`) will now show the real
`pluginBreakdowns`/`skillBreakdowns`/`sourceTypeBreakdowns` arrays with `unattributed`/`active`
entries instead of empty arrays. Run `cargo insta review` and accept these — verify each diff only
adds the three new populated arrays and does not change any other field.

- [ ] **Step 12: Commit**

```bash
git add rust/crates/ccusage-core/src/summary.rs rust/crates/ccusage-core/src/snapshots
git commit -m "feat(core): accumulate plugin/skill/source-type breakdowns in UsageAccumulator"
```

---

### Task 5: CLI flag wiring — `--by-plugin`, `--by-skill`, `--by-source-type`

**Files:**
- Modify: `rust/crates/ccusage-cli/src/types.rs`
- Modify: `rust/crates/ccusage-cli-parser/src/parser.rs`
- Modify: `rust/crates/ccusage-cli-parser/src/cli-help.json`
- Modify: `rust/crates/ccusage-cli-parser/src/tests.rs`

**Interfaces:**
- Produces: `SharedArgs.by_plugin: bool`, `SharedArgs.by_skill: bool`,
  `SharedArgs.by_source_type: bool` — consumed by Task 6 (`output.rs` table rendering gate) and
  Task 8 (`ccusage-adapter-all/report.rs` table rendering gate).

- [ ] **Step 1: Add the three new bool fields to `SharedArgs`**

Edit `rust/crates/ccusage-cli/src/types.rs` — `SharedArgs` already derives `Default` (line 33:
`#[derive(Clone, Debug, Default)]`), so the new fields default to `false` automatically. Insert
them right after `breakdown` (line 45):

```rust
pub struct SharedArgs {
    pub since: Option<String>,
    pub until: Option<String>,
    pub last: Option<u32>,
    pub json: bool,
    pub mode: CostMode,
    pub debug: bool,
    pub debug_samples: usize,
    pub order: SortOrder,
    pub breakdown: bool,
    pub by_plugin: bool,
    pub by_skill: bool,
    pub by_source_type: bool,
    pub offline: bool,
    pub no_offline: bool,
    // ...rest of the struct's existing fields unchanged...
```

(Keep every field after `offline` exactly as it exists today; only the three new lines are
inserted between `breakdown` and `offline`.)

- [ ] **Step 2: Add the three new flags to the parser's match arm**

In `rust/crates/ccusage-cli-parser/src/parser.rs`, edit `parse_shared_arg` right after the
existing `"-b" | "--breakdown" => shared.breakdown = true,` arm (line 747):

```rust
        "-b" | "--breakdown" => shared.breakdown = true,
        "--by-plugin" => shared.by_plugin = true,
        "--by-skill" => shared.by_skill = true,
        "--by-source-type" => shared.by_source_type = true,
```

- [ ] **Step 3: Add the three new flags to `is_shared_flag`**

Edit the `matches!` list in `is_shared_flag` (around line 1011-1012), right after `"--breakdown"`:

```rust
            | "-b"
            | "--breakdown"
            | "--by-plugin"
            | "--by-skill"
            | "--by-source-type"
            | "-O"
            | "--offline"
```

- [ ] **Step 4: Add the three new flags to `cli-help.json`**

In `rust/crates/ccusage-cli-parser/src/cli-help.json`, under the `shared_claude_options` array,
right after the `"-b, --breakdown"` entry (around line 301-305):

```json
		{
			"flags": "-b, --breakdown",
			"description": "Show per-model cost breakdown",
			"default": "false"
		},
		{
			"flags": "--by-plugin",
			"description": "Show per-plugin cost breakdown (Claude Code only; other agents show as unattributed)",
			"default": "false"
		},
		{
			"flags": "--by-skill",
			"description": "Show per-skill cost breakdown (Claude Code only; other agents show as unattributed)",
			"default": "false"
		},
		{
			"flags": "--by-source-type",
			"description": "Show active vs. background (sidechain) cost breakdown",
			"default": "false"
		},
```

Validate the JSON is still well-formed: run `python3 -m json.tool rust/crates/ccusage-cli-parser/src/cli-help.json > /dev/null && echo OK`.

- [ ] **Step 5: Add the three new fields to the test snapshot helper**

Edit `shared_snapshot` in `rust/crates/ccusage-cli-parser/src/tests.rs` (lines 117-134ish), right
after `"breakdown": shared.breakdown,` (line 126):

```rust
        "breakdown": shared.breakdown,
        "byPlugin": shared.by_plugin,
        "bySkill": shared.by_skill,
        "bySourceType": shared.by_source_type,
```

- [ ] **Step 6: Write a failing test parsing the three new flags**

Add a new test in `rust/crates/ccusage-cli-parser/src/tests.rs`, near the existing
`--breakdown`-covering combined test (around line 780-808):

```rust
    #[test]
    fn parses_by_plugin_by_skill_and_by_source_type_flags() {
        let cli = parse(&["ccusage", "daily", "--by-plugin", "--by-skill", "--by-source-type"])
            .unwrap();

        insta::assert_json_snapshot!(cli_snapshot(cli));
    }
```

- [ ] **Step 7: Run test to verify it fails**

Run: `cargo test -p ccusage-cli-parser parses_by_plugin_by_skill_and_by_source_type_flags`
Expected: FAIL — either a compile error (`by_plugin` not found) before Step 1, or an insta
snapshot mismatch/new-snapshot prompt after Step 1 lands but before this specific new test has an
accepted snapshot on disk. Run the steps above in order so this is a fresh new-snapshot case.

- [ ] **Step 8: Run test to verify it passes and accept its snapshot**

Run: `cargo test -p ccusage-cli-parser parses_by_plugin_by_skill_and_by_source_type_flags`
then `cargo insta review` to accept the new snapshot file. Open the accepted `.snap` file and
confirm `"byPlugin": true, "bySkill": true, "bySourceType": true` all appear.

- [ ] **Step 9: Run the full parser test suite and review remaining snapshots**

Run: `cargo test -p ccusage-cli-parser`
Expected: every other existing snapshot test that calls `shared_snapshot`/`cli_snapshot` will now
show the three new `false` fields. Run `cargo insta review` and accept — verify each diff only
adds the three new `false` fields.

- [ ] **Step 10: Commit**

```bash
git add rust/crates/ccusage-cli/src/types.rs rust/crates/ccusage-cli-parser/src/parser.rs \
  rust/crates/ccusage-cli-parser/src/cli-help.json rust/crates/ccusage-cli-parser/src/tests.rs \
  rust/crates/ccusage-cli-parser/src/snapshots
git commit -m "feat(cli): add --by-plugin, --by-skill, --by-source-type flags"
```

---

### Task 6: Single-agent rendering — `ccusage-core/src/output.rs`

**Files:**
- Modify: `rust/crates/ccusage-core/src/output.rs`
- Modify: `rust/crates/ccusage-core/src/agent_report.rs` (JSON — unconditional inclusion, no
  signature change)
- Test: `rust/crates/ccusage-core/src/output.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `SharedArgs.by_plugin/by_skill/by_source_type` from Task 5;
  `UsageSummary.plugin_breakdowns/skill_breakdowns/source_type_breakdowns` from Task 4 (and
  Task 3 for the daily fast path).
- Produces: table rows rendered under `push_plugin_breakdown_rows`,
  `push_skill_breakdown_rows`, `push_source_type_breakdown_rows` (gated by the three flags in
  `print_usage_table_with_options`), and unconditional `"pluginBreakdowns"` / `"skillBreakdowns"`
  / `"sourceTypeBreakdowns"` JSON keys in `summary_json`, `session_summary_json`, and
  `agent_summary_json` — every adapter that funnels through these shared functions (all of them)
  automatically gets both behaviors with zero further per-adapter changes.

- [ ] **Step 1: Write a failing test for table rendering gating**

Add to `rust/crates/ccusage-core/src/output.rs`'s `#[cfg(test)] mod tests`, near
`focused_table_includes_cache_creation_by_default`:

```rust
    #[test]
    fn push_plugin_breakdown_rows_emits_one_row_per_plugin_bucket() {
        let mut row = snapshot_summary("2026-01-02", None, None);
        row.plugin_breakdowns = vec![
            PluginBreakdown {
                plugin_name: "aws".to_string(),
                input_tokens: 900,
                output_tokens: 300,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                extra_total_tokens: 0,
                cost: 0.3,
                missing_pricing: false,
            },
            PluginBreakdown {
                plugin_name: "unattributed".to_string(),
                input_tokens: 334,
                output_tokens: 267,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                extra_total_tokens: 0,
                cost: 0.12,
                missing_pricing: false,
            },
        ];
        let shared = SharedArgs::default();
        let (headers, aligns) = usage_table_columns("Date", false, true);
        let mut table = SimpleTable::new(headers, aligns, TerminalStyle::default());

        push_plugin_breakdown_rows(&mut table, &row, false, true, false, &shared);

        assert_eq!(table.row_count(), 2);
    }
```

(This test's exact construction of `SimpleTable`/`TerminalStyle` and a `row_count()` accessor
must match what `ccusage-terminal`'s `SimpleTable` actually exposes — read
`rust/crates/ccusage-terminal/src/*.rs` for the real constructor signature and any row-count
accessor before finalizing this test; if no `row_count()` exists, assert on `table.print()`
output length instead, or expose/reuse whatever inspection helper the existing
`focused_table_can_omit_cache_creation_without_dropping_cache_reads`-style tests use.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ccusage-core push_plugin_breakdown_rows_emits_one_row_per_plugin_bucket`
Expected: FAIL — `push_plugin_breakdown_rows` doesn't exist yet (compile error).

- [ ] **Step 3: Import the three new breakdown types**

Edit the `use crate::{...}` block at the top of `output.rs` (lines 8-12):

```rust
use crate::{
    Align, Color, PluginBreakdown, Result, SimpleTable, SkillBreakdown, SourceTypeBreakdown,
    USAGE_COMPACT_WIDTH_THRESHOLD, UsageSummary, cli::SharedArgs, cli_error, color,
    format_project_name, parse_project_aliases, print_box_title, short_model_name, terminal_width,
};
```

(`PluginBreakdown`/`SkillBreakdown`/`SourceTypeBreakdown` are only needed if referenced by type
in this file's new functions' signatures — the functions below take `&UsageSummary` and iterate
`row.plugin_breakdowns` etc., so explicit imports of the breakdown types are only required if the
test module constructs them directly, as Step 1 does; keep the import only if the compiler
requires it.)

- [ ] **Step 4: Add `push_plugin_breakdown_rows`, `push_skill_breakdown_rows`, `push_source_type_breakdown_rows`**

Add these three functions to `output.rs`, right after `push_breakdown_rows` (after line 501):

```rust
fn push_plugin_breakdown_rows(
    table: &mut SimpleTable,
    row: &UsageSummary,
    compact: bool,
    show_cache_creation: bool,
    include_last_activity: bool,
    shared: &SharedArgs,
) {
    for breakdown in &row.plugin_breakdowns {
        let total = breakdown
            .input_tokens
            .saturating_add(breakdown.output_tokens)
            .saturating_add(breakdown.cache_creation_tokens)
            .saturating_add(breakdown.cache_read_tokens);
        let mut values = vec![
            color(
                shared,
                format!("  └─ {}", breakdown.plugin_name),
                Color::Grey,
            ),
            String::new(),
            color(shared, format_number(breakdown.input_tokens), Color::Grey),
            color(shared, format_number(breakdown.output_tokens), Color::Grey),
        ];
        if !compact && show_cache_creation {
            values.push(color(
                shared,
                format_number(breakdown.cache_creation_tokens),
                Color::Grey,
            ));
        }
        if compact {
            values.push(color(shared, format_currency(breakdown.cost), Color::Grey));
        } else {
            values.extend([
                color(
                    shared,
                    format_number(breakdown.cache_read_tokens),
                    Color::Grey,
                ),
                color(shared, format_number(total), Color::Grey),
                color(shared, format_currency(breakdown.cost), Color::Grey),
            ]);
        }
        if shared.no_cost {
            values.pop();
        }
        if include_last_activity {
            values.push(String::new());
        }
        table.push(values);
    }
}

fn push_skill_breakdown_rows(
    table: &mut SimpleTable,
    row: &UsageSummary,
    compact: bool,
    show_cache_creation: bool,
    include_last_activity: bool,
    shared: &SharedArgs,
) {
    for breakdown in &row.skill_breakdowns {
        let total = breakdown
            .input_tokens
            .saturating_add(breakdown.output_tokens)
            .saturating_add(breakdown.cache_creation_tokens)
            .saturating_add(breakdown.cache_read_tokens);
        let mut values = vec![
            color(
                shared,
                format!("  └─ {}", breakdown.skill_name),
                Color::Grey,
            ),
            String::new(),
            color(shared, format_number(breakdown.input_tokens), Color::Grey),
            color(shared, format_number(breakdown.output_tokens), Color::Grey),
        ];
        if !compact && show_cache_creation {
            values.push(color(
                shared,
                format_number(breakdown.cache_creation_tokens),
                Color::Grey,
            ));
        }
        if compact {
            values.push(color(shared, format_currency(breakdown.cost), Color::Grey));
        } else {
            values.extend([
                color(
                    shared,
                    format_number(breakdown.cache_read_tokens),
                    Color::Grey,
                ),
                color(shared, format_number(total), Color::Grey),
                color(shared, format_currency(breakdown.cost), Color::Grey),
            ]);
        }
        if shared.no_cost {
            values.pop();
        }
        if include_last_activity {
            values.push(String::new());
        }
        table.push(values);
    }
}

fn push_source_type_breakdown_rows(
    table: &mut SimpleTable,
    row: &UsageSummary,
    compact: bool,
    show_cache_creation: bool,
    include_last_activity: bool,
    shared: &SharedArgs,
) {
    for breakdown in &row.source_type_breakdowns {
        let total = breakdown
            .input_tokens
            .saturating_add(breakdown.output_tokens)
            .saturating_add(breakdown.cache_creation_tokens)
            .saturating_add(breakdown.cache_read_tokens);
        let mut values = vec![
            color(
                shared,
                format!("  └─ {}", breakdown.source_type),
                Color::Grey,
            ),
            String::new(),
            color(shared, format_number(breakdown.input_tokens), Color::Grey),
            color(shared, format_number(breakdown.output_tokens), Color::Grey),
        ];
        if !compact && show_cache_creation {
            values.push(color(
                shared,
                format_number(breakdown.cache_creation_tokens),
                Color::Grey,
            ));
        }
        if compact {
            values.push(color(shared, format_currency(breakdown.cost), Color::Grey));
        } else {
            values.extend([
                color(
                    shared,
                    format_number(breakdown.cache_read_tokens),
                    Color::Grey,
                ),
                color(shared, format_number(total), Color::Grey),
                color(shared, format_currency(breakdown.cost), Color::Grey),
            ]);
        }
        if shared.no_cost {
            values.pop();
        }
        if include_last_activity {
            values.push(String::new());
        }
        table.push(values);
    }
}
```

- [ ] **Step 5: Gate the three new render calls behind their flags in `print_usage_table_with_options`**

Edit the loop body in `print_usage_table_with_options` (lines 234-295), right after the existing
`if shared.breakdown { push_breakdown_rows(...); }` block:

```rust
        if shared.breakdown {
            push_breakdown_rows(
                &mut table,
                row,
                compact,
                options.show_cache_creation,
                include_last_activity,
                shared,
            );
        }
        if shared.by_plugin {
            push_plugin_breakdown_rows(
                &mut table,
                row,
                compact,
                options.show_cache_creation,
                include_last_activity,
                shared,
            );
        }
        if shared.by_skill {
            push_skill_breakdown_rows(
                &mut table,
                row,
                compact,
                options.show_cache_creation,
                include_last_activity,
                shared,
            );
        }
        if shared.by_source_type {
            push_source_type_breakdown_rows(
                &mut table,
                row,
                compact,
                options.show_cache_creation,
                include_last_activity,
                shared,
            );
        }
```

- [ ] **Step 6: Run the table test**

Adjust Step 1's test if the real `SimpleTable` API differs from the guess above, then run:
Run: `cargo test -p ccusage-core push_plugin_breakdown_rows_emits_one_row_per_plugin_bucket`
Expected: PASS.

- [ ] **Step 7: Add the three new keys to `summary_json` and `session_summary_json` (unconditional, mirroring `modelBreakdowns`)**

Edit `summary_json` (lines 27-56):

```rust
pub fn summary_json(row: &UsageSummary) -> Value {
    let mut value = json!({
        "inputTokens": row.input_tokens,
        "outputTokens": row.output_tokens,
        "cacheCreationTokens": row.cache_creation_tokens,
        "cacheReadTokens": row.cache_read_tokens,
        "totalTokens": row.total_tokens(),
        "totalCost": row.total_cost,
        "modelsUsed": row.models_used,
        "modelBreakdowns": row.model_breakdowns,
        "pluginBreakdowns": row.plugin_breakdowns,
        "skillBreakdowns": row.skill_breakdowns,
        "sourceTypeBreakdowns": row.source_type_breakdowns,
    });
    // ...rest of the function body unchanged...
```

Edit `session_summary_json` (lines 58-77) the same way, adding the three keys right after
`"modelBreakdowns": row.model_breakdowns,`:

```rust
pub fn session_summary_json(row: &UsageSummary) -> Value {
    let mut value = json!({
        "sessionId": row.session_id,
        "inputTokens": row.input_tokens,
        "outputTokens": row.output_tokens,
        "cacheCreationTokens": row.cache_creation_tokens,
        "cacheReadTokens": row.cache_read_tokens,
        "totalTokens": row.total_tokens(),
        "totalCost": row.total_cost,
        "lastActivity": row.last_activity,
        "firstActivity": row.first_activity,
        "modelsUsed": row.models_used,
        "modelBreakdowns": row.model_breakdowns,
        "pluginBreakdowns": row.plugin_breakdowns,
        "skillBreakdowns": row.skill_breakdowns,
        "sourceTypeBreakdowns": row.source_type_breakdowns,
        "projectPath": row.project_path,
    });
    // ...rest of the function body unchanged...
```

- [ ] **Step 8: Add the same three keys to `agent_summary_json` in `agent_report.rs`**

Read `rust/crates/ccusage-core/src/agent_report.rs`'s `agent_summary_json` function fully (it
builds a `json!({ ... "modelBreakdowns": row.model_breakdowns, ... })` literal, the same shape as
`summary_json`). Add `"pluginBreakdowns": row.plugin_breakdowns,`, `"skillBreakdowns":
row.skill_breakdowns,`, `"sourceTypeBreakdowns": row.source_type_breakdowns,` immediately after
its `"modelBreakdowns": row.model_breakdowns,` line, keeping every other field in that literal
unchanged. This single edit propagates to all 17+ adapters that call `agent_summary_json` without
touching any of their individual `report.rs` files.

- [ ] **Step 9: Update the snapshot test fixture builder with sample data**

Edit `snapshot_summary` (the test helper at the bottom of `output.rs`'s test module, around line
820-866) to populate one entry each in `plugin_breakdowns`, `skill_breakdowns`, and
`source_type_breakdowns`, e.g. right after the `model_breakdowns` field:

```rust
            plugin_breakdowns: vec![PluginBreakdown {
                plugin_name: "aws".to_string(),
                input_tokens: 900,
                output_tokens: 300,
                cache_creation_tokens: 50,
                cache_read_tokens: 10,
                extra_total_tokens: 0,
                cost: 0.3,
                missing_pricing: false,
            }],
            skill_breakdowns: vec![SkillBreakdown {
                skill_name: "superpowers:brainstorming".to_string(),
                input_tokens: 900,
                output_tokens: 300,
                cache_creation_tokens: 50,
                cache_read_tokens: 10,
                extra_total_tokens: 0,
                cost: 0.3,
                missing_pricing: false,
            }],
            source_type_breakdowns: vec![SourceTypeBreakdown {
                source_type: "active".to_string(),
                input_tokens: 1234,
                output_tokens: 567,
                cache_creation_tokens: 89,
                cache_read_tokens: 10,
                extra_total_tokens: 0,
                cost: 0.42,
                missing_pricing: false,
            }],
```

- [ ] **Step 10: Run the output.rs test suite and review snapshots**

Run: `cargo test -p ccusage-core --lib output::`
Expected: `snapshots_summary_json_with_optional_fields_and_model_breakdowns` and
`snapshots_session_summary_json_with_present_and_missing_options` will now include the three new
populated arrays. Run `cargo insta review` and accept — confirm the diffs show exactly the three
new arrays with the sample data from Step 9.

- [ ] **Step 11: Run the full workspace test suite**

Run: `cargo test --workspace`
Expected: PASS (adapters that call `agent_summary_json` will also see new snapshot diffs if they
snapshot its output directly — accept those the same way, confirming only the three new arrays
appear).

- [ ] **Step 12: Commit**

```bash
git add rust/crates/ccusage-core/src/output.rs rust/crates/ccusage-core/src/agent_report.rs \
  rust/crates/ccusage-core/src/snapshots
git commit -m "feat(core): render plugin/skill/source-type breakdowns in table and JSON output"
```

---

### Task 7: Cross-agent aggregation — `ccusage-adapter-all/src/types.rs`

**Files:**
- Modify: `rust/crates/ccusage-adapter-all/src/types.rs`
- Modify: `rust/crates/ccusage-adapter-all/src/loader.rs:791` (construction site)
- Test: `rust/crates/ccusage-adapter-all/src/types.rs` or `tests.rs`

**Interfaces:**
- Consumes: `UsageSummary.plugin_breakdowns/skill_breakdowns/source_type_breakdowns` from Tasks
  3-4; `PluginBreakdown`/`SkillBreakdown`/`SourceTypeBreakdown` from Task 1.
- Produces: `AllRow.plugin_breakdowns: Vec<PluginBreakdown>`,
  `AllRow.skill_breakdowns: Vec<SkillBreakdown>`,
  `AllRow.source_type_breakdowns: Vec<SourceTypeBreakdown>` — consumed by Task 8.

- [ ] **Step 1: Import the three new breakdown types**

Edit the `use crate::{...}` line at the top of
`rust/crates/ccusage-adapter-all/src/types.rs` (line 5):

```rust
use crate::{ModelBreakdown, PluginBreakdown, SkillBreakdown, SourceTypeBreakdown, cli::AgentReportKind, fast::FxHashMap};
```

- [ ] **Step 2: Add the three new fields to `AllRow`**

Edit `AllRow` (lines 8-22), right after `model_breakdowns`:

```rust
#[derive(Debug, Clone)]
pub(super) struct AllRow {
    pub(super) period: String,
    pub(super) agent: &'static str,
    pub(super) models_used: Vec<String>,
    pub(super) input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) cache_creation_tokens: u64,
    pub(super) cache_read_tokens: u64,
    pub(super) total_tokens: u64,
    pub(super) total_cost: f64,
    pub(super) metadata: Option<Value>,
    pub(super) metadata_agents: Option<Vec<&'static str>>,
    pub(super) agent_breakdowns: Option<Vec<AllRow>>,
    pub(super) model_breakdowns: Vec<ModelBreakdown>,
    pub(super) plugin_breakdowns: Vec<PluginBreakdown>,
    pub(super) skill_breakdowns: Vec<SkillBreakdown>,
    pub(super) source_type_breakdowns: Vec<SourceTypeBreakdown>,
}
```

- [ ] **Step 3: Fix every `AllRow { ... }` literal across the crate**

Run: `cargo build -p ccusage-adapter-all 2>&1 | grep -B2 "missing field"`

For every reported literal in `types.rs`, `loader.rs`, and `tests.rs`, add
`plugin_breakdowns: Vec::new(), skill_breakdowns: Vec::new(), source_type_breakdowns: Vec::new(),`
right after `model_breakdowns: ...` in each. (Note: `AllAccumulator::add`'s struct-update-syntax
literal at types.rs line 99-103 — `AllRow { metadata_agents: Some(vec![row.agent]),
agent_breakdowns: None, ..row }` — uses `..row` spread syntax and needs **no edit**, since the
three new fields are already present on `row` and carry through automatically.)

- [ ] **Step 4: Wire the real values at the `summary_rows` construction site in `loader.rs`**

Edit `rust/crates/ccusage-adapter-all/src/loader.rs`'s `summary_rows` function, at the `AllRow`
literal around line 778-792, right after `model_breakdowns: summary.model_breakdowns,`:

```rust
                model_breakdowns: summary.model_breakdowns,
                plugin_breakdowns: summary.plugin_breakdowns,
                skill_breakdowns: summary.skill_breakdowns,
                source_type_breakdowns: summary.source_type_breakdowns,
```

- [ ] **Step 5: Write a failing test for `merge_agent_breakdown` and `AllAccumulator::into_row`**

Add to `rust/crates/ccusage-adapter-all/src/types.rs`'s test module (or `tests.rs`, matching
wherever `AllAccumulator`/`merge_model_breakdowns` are already tested — grep for
`merge_model_breakdowns` test coverage first to place this alongside it):

```rust
    #[test]
    fn merges_and_aggregates_plugin_skill_and_source_type_breakdowns_across_agents() {
        let mut accumulator = AllAccumulator::default();
        accumulator.add(AllRow {
            period: "2026-01-02".to_string(),
            agent: "claude",
            models_used: vec!["claude-sonnet-4-20250514".to_string()],
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            total_tokens: 150,
            total_cost: 0.1,
            metadata: None,
            metadata_agents: None,
            agent_breakdowns: None,
            model_breakdowns: Vec::new(),
            plugin_breakdowns: vec![PluginBreakdown {
                plugin_name: "aws".to_string(),
                input_tokens: 100,
                output_tokens: 50,
                cost: 0.1,
                ..PluginBreakdown::default()
            }],
            skill_breakdowns: Vec::new(),
            source_type_breakdowns: vec![SourceTypeBreakdown {
                source_type: "active".to_string(),
                input_tokens: 100,
                output_tokens: 50,
                cost: 0.1,
                ..SourceTypeBreakdown::default()
            }],
        });
        accumulator.add(AllRow {
            period: "2026-01-02".to_string(),
            agent: "codex",
            models_used: vec!["gpt-5.2-codex".to_string()],
            input_tokens: 20,
            output_tokens: 10,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            total_tokens: 30,
            total_cost: 0.02,
            metadata: None,
            metadata_agents: None,
            agent_breakdowns: None,
            model_breakdowns: Vec::new(),
            plugin_breakdowns: vec![PluginBreakdown {
                plugin_name: "unattributed".to_string(),
                input_tokens: 20,
                output_tokens: 10,
                cost: 0.02,
                ..PluginBreakdown::default()
            }],
            skill_breakdowns: Vec::new(),
            source_type_breakdowns: vec![SourceTypeBreakdown {
                source_type: "active".to_string(),
                input_tokens: 20,
                output_tokens: 10,
                cost: 0.02,
                ..SourceTypeBreakdown::default()
            }],
        });

        let row = accumulator.into_row("2026-01-02".to_string());

        assert_eq!(row.plugin_breakdowns.len(), 2);
        assert!(
            row.plugin_breakdowns
                .iter()
                .any(|b| b.plugin_name == "aws" && b.input_tokens == 100)
        );
        assert_eq!(row.source_type_breakdowns.len(), 1);
        assert_eq!(row.source_type_breakdowns[0].input_tokens, 120);
    }
```

- [ ] **Step 6: Run test to verify it fails**

Run: `cargo test -p ccusage-adapter-all merges_and_aggregates_plugin_skill_and_source_type_breakdowns_across_agents`
Expected: FAIL — `row.plugin_breakdowns` is empty since `into_row`/`merge_agent_breakdown` don't
populate it yet.

- [ ] **Step 7: Add merge logic for the three new dimensions in `merge_agent_breakdown`**

Edit `merge_agent_breakdown` (lines 134-150):

```rust
fn merge_agent_breakdown(target: &mut AllRow, source: AllRow) {
    target.input_tokens = target.input_tokens.saturating_add(source.input_tokens);
    target.output_tokens = target.output_tokens.saturating_add(source.output_tokens);
    target.cache_creation_tokens = target
        .cache_creation_tokens
        .saturating_add(source.cache_creation_tokens);
    target.cache_read_tokens = target
        .cache_read_tokens
        .saturating_add(source.cache_read_tokens);
    target.total_tokens = target.total_tokens.saturating_add(source.total_tokens);
    target.total_cost += source.total_cost;
    let mut models: BTreeSet<String> = target.models_used.drain(..).collect();
    models.extend(source.models_used);
    target.models_used = models.into_iter().collect();
    target.model_breakdowns =
        merge_model_breakdowns(target.model_breakdowns.drain(..), source.model_breakdowns);
    target.plugin_breakdowns = merge_plugin_breakdowns(
        target.plugin_breakdowns.drain(..),
        source.plugin_breakdowns,
    );
    target.skill_breakdowns =
        merge_skill_breakdowns(target.skill_breakdowns.drain(..), source.skill_breakdowns);
    target.source_type_breakdowns = merge_source_type_breakdowns(
        target.source_type_breakdowns.drain(..),
        source.source_type_breakdowns,
    );
}
```

- [ ] **Step 8: Add `merge_plugin_breakdowns`, `merge_skill_breakdowns`, `merge_source_type_breakdowns`**

Add these three functions right after `merge_model_breakdowns` (after line 180):

```rust
fn merge_plugin_breakdowns(
    existing: impl IntoIterator<Item = PluginBreakdown>,
    additional: impl IntoIterator<Item = PluginBreakdown>,
) -> Vec<PluginBreakdown> {
    let mut indexes = FxHashMap::<String, usize>::default();
    let mut breakdowns: Vec<PluginBreakdown> = Vec::new();
    for item in existing.into_iter().chain(additional) {
        let index = *indexes.entry(item.plugin_name.clone()).or_insert_with(|| {
            let i = breakdowns.len();
            breakdowns.push(PluginBreakdown {
                plugin_name: item.plugin_name.clone(),
                ..PluginBreakdown::default()
            });
            i
        });
        let b = &mut breakdowns[index];
        b.input_tokens = b.input_tokens.saturating_add(item.input_tokens);
        b.output_tokens = b.output_tokens.saturating_add(item.output_tokens);
        b.cache_creation_tokens = b
            .cache_creation_tokens
            .saturating_add(item.cache_creation_tokens);
        b.cache_read_tokens = b.cache_read_tokens.saturating_add(item.cache_read_tokens);
        b.extra_total_tokens = b.extra_total_tokens.saturating_add(item.extra_total_tokens);
        b.cost += item.cost;
        b.missing_pricing |= item.missing_pricing;
    }
    breakdowns.sort_by(|a, b| b.cost.total_cmp(&a.cost));
    breakdowns
}

fn merge_skill_breakdowns(
    existing: impl IntoIterator<Item = SkillBreakdown>,
    additional: impl IntoIterator<Item = SkillBreakdown>,
) -> Vec<SkillBreakdown> {
    let mut indexes = FxHashMap::<String, usize>::default();
    let mut breakdowns: Vec<SkillBreakdown> = Vec::new();
    for item in existing.into_iter().chain(additional) {
        let index = *indexes.entry(item.skill_name.clone()).or_insert_with(|| {
            let i = breakdowns.len();
            breakdowns.push(SkillBreakdown {
                skill_name: item.skill_name.clone(),
                ..SkillBreakdown::default()
            });
            i
        });
        let b = &mut breakdowns[index];
        b.input_tokens = b.input_tokens.saturating_add(item.input_tokens);
        b.output_tokens = b.output_tokens.saturating_add(item.output_tokens);
        b.cache_creation_tokens = b
            .cache_creation_tokens
            .saturating_add(item.cache_creation_tokens);
        b.cache_read_tokens = b.cache_read_tokens.saturating_add(item.cache_read_tokens);
        b.extra_total_tokens = b.extra_total_tokens.saturating_add(item.extra_total_tokens);
        b.cost += item.cost;
        b.missing_pricing |= item.missing_pricing;
    }
    breakdowns.sort_by(|a, b| b.cost.total_cmp(&a.cost));
    breakdowns
}

fn merge_source_type_breakdowns(
    existing: impl IntoIterator<Item = SourceTypeBreakdown>,
    additional: impl IntoIterator<Item = SourceTypeBreakdown>,
) -> Vec<SourceTypeBreakdown> {
    let mut indexes = FxHashMap::<String, usize>::default();
    let mut breakdowns: Vec<SourceTypeBreakdown> = Vec::new();
    for item in existing.into_iter().chain(additional) {
        let index = *indexes.entry(item.source_type.clone()).or_insert_with(|| {
            let i = breakdowns.len();
            breakdowns.push(SourceTypeBreakdown {
                source_type: item.source_type.clone(),
                ..SourceTypeBreakdown::default()
            });
            i
        });
        let b = &mut breakdowns[index];
        b.input_tokens = b.input_tokens.saturating_add(item.input_tokens);
        b.output_tokens = b.output_tokens.saturating_add(item.output_tokens);
        b.cache_creation_tokens = b
            .cache_creation_tokens
            .saturating_add(item.cache_creation_tokens);
        b.cache_read_tokens = b.cache_read_tokens.saturating_add(item.cache_read_tokens);
        b.extra_total_tokens = b.extra_total_tokens.saturating_add(item.extra_total_tokens);
        b.cost += item.cost;
        b.missing_pricing |= item.missing_pricing;
    }
    breakdowns.sort_by(|a, b| b.cost.total_cmp(&a.cost));
    breakdowns
}
```

- [ ] **Step 9: Add `aggregate_plugin_breakdowns`, `aggregate_skill_breakdowns`, `aggregate_source_type_breakdowns`**

Add these three functions right after `aggregate_model_breakdowns` (after line 208):

```rust
fn aggregate_plugin_breakdowns(rows: &[AllRow]) -> Vec<PluginBreakdown> {
    let mut indexes = FxHashMap::<String, usize>::default();
    let mut breakdowns: Vec<PluginBreakdown> = Vec::new();
    for row in rows {
        for item in &row.plugin_breakdowns {
            let index = *indexes.entry(item.plugin_name.clone()).or_insert_with(|| {
                let i = breakdowns.len();
                breakdowns.push(PluginBreakdown {
                    plugin_name: item.plugin_name.clone(),
                    ..PluginBreakdown::default()
                });
                i
            });
            let b = &mut breakdowns[index];
            b.input_tokens = b.input_tokens.saturating_add(item.input_tokens);
            b.output_tokens = b.output_tokens.saturating_add(item.output_tokens);
            b.cache_creation_tokens = b
                .cache_creation_tokens
                .saturating_add(item.cache_creation_tokens);
            b.cache_read_tokens = b.cache_read_tokens.saturating_add(item.cache_read_tokens);
            b.extra_total_tokens = b.extra_total_tokens.saturating_add(item.extra_total_tokens);
            b.cost += item.cost;
            b.missing_pricing |= item.missing_pricing;
        }
    }
    breakdowns
}

fn aggregate_skill_breakdowns(rows: &[AllRow]) -> Vec<SkillBreakdown> {
    let mut indexes = FxHashMap::<String, usize>::default();
    let mut breakdowns: Vec<SkillBreakdown> = Vec::new();
    for row in rows {
        for item in &row.skill_breakdowns {
            let index = *indexes.entry(item.skill_name.clone()).or_insert_with(|| {
                let i = breakdowns.len();
                breakdowns.push(SkillBreakdown {
                    skill_name: item.skill_name.clone(),
                    ..SkillBreakdown::default()
                });
                i
            });
            let b = &mut breakdowns[index];
            b.input_tokens = b.input_tokens.saturating_add(item.input_tokens);
            b.output_tokens = b.output_tokens.saturating_add(item.output_tokens);
            b.cache_creation_tokens = b
                .cache_creation_tokens
                .saturating_add(item.cache_creation_tokens);
            b.cache_read_tokens = b.cache_read_tokens.saturating_add(item.cache_read_tokens);
            b.extra_total_tokens = b.extra_total_tokens.saturating_add(item.extra_total_tokens);
            b.cost += item.cost;
            b.missing_pricing |= item.missing_pricing;
        }
    }
    breakdowns
}

fn aggregate_source_type_breakdowns(rows: &[AllRow]) -> Vec<SourceTypeBreakdown> {
    let mut indexes = FxHashMap::<String, usize>::default();
    let mut breakdowns: Vec<SourceTypeBreakdown> = Vec::new();
    for row in rows {
        for item in &row.source_type_breakdowns {
            let index = *indexes.entry(item.source_type.clone()).or_insert_with(|| {
                let i = breakdowns.len();
                breakdowns.push(SourceTypeBreakdown {
                    source_type: item.source_type.clone(),
                    ..SourceTypeBreakdown::default()
                });
                i
            });
            let b = &mut breakdowns[index];
            b.input_tokens = b.input_tokens.saturating_add(item.input_tokens);
            b.output_tokens = b.output_tokens.saturating_add(item.output_tokens);
            b.cache_creation_tokens = b
                .cache_creation_tokens
                .saturating_add(item.cache_creation_tokens);
            b.cache_read_tokens = b.cache_read_tokens.saturating_add(item.cache_read_tokens);
            b.extra_total_tokens = b.extra_total_tokens.saturating_add(item.extra_total_tokens);
            b.cost += item.cost;
            b.missing_pricing |= item.missing_pricing;
        }
    }
    breakdowns
}
```

- [ ] **Step 10: Wire the three new aggregate calls into `AllAccumulator::into_row`**

Edit `into_row` (lines 108-131):

```rust
    pub(super) fn into_row(self, period: String) -> AllRow {
        let mut agent_breakdowns = self.agent_breakdowns;
        for breakdown in &mut agent_breakdowns {
            breakdown.period = period.clone();
        }
        agent_breakdowns.sort_by(|a, b| a.agent.cmp(b.agent));
        let mut model_breakdowns = aggregate_model_breakdowns(&agent_breakdowns);
        model_breakdowns.sort_by(|a, b| b.cost.total_cmp(&a.cost));
        let mut plugin_breakdowns = aggregate_plugin_breakdowns(&agent_breakdowns);
        plugin_breakdowns.sort_by(|a, b| b.cost.total_cmp(&a.cost));
        let mut skill_breakdowns = aggregate_skill_breakdowns(&agent_breakdowns);
        skill_breakdowns.sort_by(|a, b| b.cost.total_cmp(&a.cost));
        let mut source_type_breakdowns = aggregate_source_type_breakdowns(&agent_breakdowns);
        source_type_breakdowns.sort_by(|a, b| b.cost.total_cmp(&a.cost));
        AllRow {
            period,
            // ...existing fields (agent, models_used, input_tokens, etc.) unchanged...
            agent_breakdowns: Some(agent_breakdowns),
            model_breakdowns,
            plugin_breakdowns,
            skill_breakdowns,
            source_type_breakdowns,
        }
    }
```

(Read the existing body between `period,` and `agent_breakdowns: Some(agent_breakdowns),` first —
those middle fields, e.g. `agent: "all"`, token totals from `self.*`, are untouched; only the
three new `let` bindings and their three new struct-literal lines are added.)

- [ ] **Step 11: Run test to verify it passes**

Run: `cargo test -p ccusage-adapter-all merges_and_aggregates_plugin_skill_and_source_type_breakdowns_across_agents`
Expected: PASS.

- [ ] **Step 12: Run the full crate test suite**

Run: `cargo test -p ccusage-adapter-all`
Expected: PASS (fix any other broken `AllRow` literal from Step 3 not yet caught).

- [ ] **Step 13: Commit**

```bash
git add rust/crates/ccusage-adapter-all/src/types.rs rust/crates/ccusage-adapter-all/src/loader.rs
git commit -m "feat(adapter-all): merge and aggregate plugin/skill/source-type breakdowns across agents"
```

---

### Task 8: Cross-agent rendering — `ccusage-adapter-all/src/report.rs`

**Files:**
- Modify: `rust/crates/ccusage-adapter-all/src/report.rs`
- Test: `rust/crates/ccusage-adapter-all/src/report.rs` (`#[cfg(test)] mod tests`) or `tests.rs`

**Interfaces:**
- Consumes: `AllRow.plugin_breakdowns/skill_breakdowns/source_type_breakdowns` from Task 7;
  `SharedArgs.by_plugin/by_skill/by_source_type` from Task 5.
- Produces: `"pluginBreakdowns"`/`"skillBreakdowns"`/`"sourceTypeBreakdowns"` keys in
  `agent_json` (unconditional JSON, mirroring `"modelBreakdowns"`); gated table rows via
  `push_plugin_breakdown_rows`/`push_skill_breakdown_rows`/`push_source_type_breakdown_rows` in
  `print_table` (mirroring the `shared.breakdown` gate).

- [ ] **Step 1: Add the three new keys to `agent_json` (unconditional)**

Edit `agent_json` in `rust/crates/ccusage-adapter-all/src/report.rs` (lines 159-171):

```rust
fn agent_json(row: &AllRow) -> Value {
    json!({
        "agent": row.agent,
        "modelsUsed": row.models_used,
        "inputTokens": row.input_tokens,
        "outputTokens": row.output_tokens,
        "cacheCreationTokens": row.cache_creation_tokens,
        "cacheReadTokens": row.cache_read_tokens,
        "totalTokens": row.total_tokens,
        "totalCost": json_float(row.total_cost),
        "modelBreakdowns": row.model_breakdowns,
        "pluginBreakdowns": row.plugin_breakdowns,
        "skillBreakdowns": row.skill_breakdowns,
        "sourceTypeBreakdowns": row.source_type_breakdowns,
    })
}
```

- [ ] **Step 2: Write a failing test asserting non-claude rows show unattributed/active defaults in JSON**

Add to the test module (grep for where `agent_json`/`report_json` are already tested, likely in
`report.rs` itself under `#[cfg(test)]` or in `tests.rs`):

```rust
    #[test]
    fn non_claude_agent_rows_show_unattributed_and_active_defaults_in_json() {
        let row = AllRow {
            period: "2026-01-02".to_string(),
            agent: "codex",
            models_used: vec!["gpt-5.2-codex".to_string()],
            input_tokens: 20,
            output_tokens: 10,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            total_tokens: 30,
            total_cost: 0.02,
            metadata: None,
            metadata_agents: None,
            agent_breakdowns: None,
            model_breakdowns: Vec::new(),
            plugin_breakdowns: vec![PluginBreakdown {
                plugin_name: "unattributed".to_string(),
                input_tokens: 20,
                output_tokens: 10,
                cost: 0.02,
                ..PluginBreakdown::default()
            }],
            skill_breakdowns: vec![SkillBreakdown {
                skill_name: "unattributed".to_string(),
                input_tokens: 20,
                output_tokens: 10,
                cost: 0.02,
                ..SkillBreakdown::default()
            }],
            source_type_breakdowns: vec![SourceTypeBreakdown {
                source_type: "active".to_string(),
                input_tokens: 20,
                output_tokens: 10,
                cost: 0.02,
                ..SourceTypeBreakdown::default()
            }],
        };

        let value = agent_json(&row);

        assert_eq!(
            value["pluginBreakdowns"][0]["pluginName"],
            serde_json::json!("unattributed")
        );
        assert_eq!(
            value["skillBreakdowns"][0]["skillName"],
            serde_json::json!("unattributed")
        );
        assert_eq!(
            value["sourceTypeBreakdowns"][0]["sourceType"],
            serde_json::json!("active")
        );
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p ccusage-adapter-all non_claude_agent_rows_show_unattributed_and_active_defaults_in_json`
Expected: FAIL — compile error (`"pluginBreakdowns"` key not present) before Step 1 lands, or PASS
immediately after Step 1 if run after. Run this after Step 1's edit to keep TDD order: write the
test first (it won't compile against the pre-edit `AllRow` literal missing fields — that failure
counts as the required "red" step), then make Step 1's edit, then re-run to see it go green.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p ccusage-adapter-all non_claude_agent_rows_show_unattributed_and_active_defaults_in_json`
Expected: PASS.

- [ ] **Step 5: Add `push_plugin_breakdown_rows`, `push_skill_breakdown_rows`, `push_source_type_breakdown_rows`**

Add these three functions to `report.rs`, right after `push_model_breakdown_rows` (find its exact
end — it starts at line 469; read through to its closing brace first):

```rust
fn push_plugin_breakdown_rows(
    table: &mut SimpleTable,
    breakdowns: &[PluginBreakdown],
    compact: bool,
    shared: &SharedArgs,
) {
    for breakdown in breakdowns {
        let total = breakdown
            .input_tokens
            .saturating_add(breakdown.output_tokens)
            .saturating_add(breakdown.cache_creation_tokens)
            .saturating_add(breakdown.cache_read_tokens);
        let mut values = vec![
            color(
                shared,
                format!("    └─ {}", breakdown.plugin_name),
                Color::Grey,
            ),
            String::new(),
            String::new(),
            color(shared, format_number(breakdown.input_tokens), Color::Grey),
            color(shared, format_number(breakdown.output_tokens), Color::Grey),
        ];
        if compact {
            values.push(color(shared, format_currency(breakdown.cost), Color::Grey));
        } else {
            values.extend([
                color(
                    shared,
                    format_number(breakdown.cache_read_tokens),
                    Color::Grey,
                ),
                color(shared, format_number(total), Color::Grey),
                color(shared, format_currency(breakdown.cost), Color::Grey),
            ]);
        }
        if shared.no_cost {
            values.pop();
        }
        table.push(values);
    }
}

fn push_skill_breakdown_rows(
    table: &mut SimpleTable,
    breakdowns: &[SkillBreakdown],
    compact: bool,
    shared: &SharedArgs,
) {
    for breakdown in breakdowns {
        let total = breakdown
            .input_tokens
            .saturating_add(breakdown.output_tokens)
            .saturating_add(breakdown.cache_creation_tokens)
            .saturating_add(breakdown.cache_read_tokens);
        let mut values = vec![
            color(
                shared,
                format!("    └─ {}", breakdown.skill_name),
                Color::Grey,
            ),
            String::new(),
            String::new(),
            color(shared, format_number(breakdown.input_tokens), Color::Grey),
            color(shared, format_number(breakdown.output_tokens), Color::Grey),
        ];
        if compact {
            values.push(color(shared, format_currency(breakdown.cost), Color::Grey));
        } else {
            values.extend([
                color(
                    shared,
                    format_number(breakdown.cache_read_tokens),
                    Color::Grey,
                ),
                color(shared, format_number(total), Color::Grey),
                color(shared, format_currency(breakdown.cost), Color::Grey),
            ]);
        }
        if shared.no_cost {
            values.pop();
        }
        table.push(values);
    }
}

fn push_source_type_breakdown_rows(
    table: &mut SimpleTable,
    breakdowns: &[SourceTypeBreakdown],
    compact: bool,
    shared: &SharedArgs,
) {
    for breakdown in breakdowns {
        let total = breakdown
            .input_tokens
            .saturating_add(breakdown.output_tokens)
            .saturating_add(breakdown.cache_creation_tokens)
            .saturating_add(breakdown.cache_read_tokens);
        let mut values = vec![
            color(
                shared,
                format!("    └─ {}", breakdown.source_type),
                Color::Grey,
            ),
            String::new(),
            String::new(),
            color(shared, format_number(breakdown.input_tokens), Color::Grey),
            color(shared, format_number(breakdown.output_tokens), Color::Grey),
        ];
        if compact {
            values.push(color(shared, format_currency(breakdown.cost), Color::Grey));
        } else {
            values.extend([
                color(
                    shared,
                    format_number(breakdown.cache_read_tokens),
                    Color::Grey,
                ),
                color(shared, format_number(total), Color::Grey),
                color(shared, format_currency(breakdown.cost), Color::Grey),
            ]);
        }
        if shared.no_cost {
            values.pop();
        }
        table.push(values);
    }
}
```

Read `push_model_breakdown_rows`'s exact parameter list first (this plan assumes it takes
`(&mut SimpleTable, &[ModelBreakdown], bool, &SharedArgs)` based on its two call sites at lines
238-243 and 247 — `push_model_breakdown_rows(&mut table, &breakdown.model_breakdowns, compact,
shared)` — adjust the three new functions' signatures to match exactly if the real signature
differs, e.g. if it also threads `include_agents`/nesting depth for the indent prefix (the
`"    └─ "` four-space indent above assumes agent-breakdown-nested model rows use a deeper indent
than the single-agent `output.rs` version's two-space indent — confirm the real indent string used
by `push_model_breakdown_rows` and match it exactly).

- [ ] **Step 6: Gate the three new render calls behind their flags in `print_table`**

Edit the loop body in `print_table` (lines 232-249):

```rust
    for row in rows {
        table.push(all_table_row(row, compact, false, shared.no_cost));
        if let Some(agent_breakdowns) = row.agent_breakdowns.as_ref() {
            for breakdown in agent_breakdowns {
                table.push(all_table_row(breakdown, compact, true, shared.no_cost));
                if shared.breakdown && !breakdown.model_breakdowns.is_empty() {
                    push_model_breakdown_rows(
                        &mut table,
                        &breakdown.model_breakdowns,
                        compact,
                        shared,
                    );
                }
                if shared.by_plugin && !breakdown.plugin_breakdowns.is_empty() {
                    push_plugin_breakdown_rows(
                        &mut table,
                        &breakdown.plugin_breakdowns,
                        compact,
                        shared,
                    );
                }
                if shared.by_skill && !breakdown.skill_breakdowns.is_empty() {
                    push_skill_breakdown_rows(
                        &mut table,
                        &breakdown.skill_breakdowns,
                        compact,
                        shared,
                    );
                }
                if shared.by_source_type && !breakdown.source_type_breakdowns.is_empty() {
                    push_source_type_breakdown_rows(
                        &mut table,
                        &breakdown.source_type_breakdowns,
                        compact,
                        shared,
                    );
                }
            }
        } else {
            if shared.breakdown && !row.model_breakdowns.is_empty() {
                push_model_breakdown_rows(&mut table, &row.model_breakdowns, compact, shared);
            }
            if shared.by_plugin && !row.plugin_breakdowns.is_empty() {
                push_plugin_breakdown_rows(&mut table, &row.plugin_breakdowns, compact, shared);
            }
            if shared.by_skill && !row.skill_breakdowns.is_empty() {
                push_skill_breakdown_rows(&mut table, &row.skill_breakdowns, compact, shared);
            }
            if shared.by_source_type && !row.source_type_breakdowns.is_empty() {
                push_source_type_breakdown_rows(
                    &mut table,
                    &row.source_type_breakdowns,
                    compact,
                    shared,
                );
            }
        }
    }
```

- [ ] **Step 7: Run the report.rs test suite**

Run: `cargo test -p ccusage-adapter-all --lib report::`
Expected: PASS.

- [ ] **Step 8: Run the full crate test suite and review snapshots**

Run: `cargo test -p ccusage-adapter-all`
Expected: any existing `report_json`/`sections_report_json` snapshot tests will now show the three
new arrays for every row. Run `cargo insta review` and accept, confirming diffs only add the new
arrays.

- [ ] **Step 9: Run the full workspace test suite**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add rust/crates/ccusage-adapter-all/src/report.rs rust/crates/ccusage-adapter-all/src/snapshots
git commit -m "feat(adapter-all): render plugin/skill/source-type breakdowns in cross-agent report"
```

---

### Task 9: Documentation

**Files:**
- Modify: `apps/ccusage/README.md`
- Modify: `docs/guide/cli-options.md`

**Interfaces:**
- Consumes: nothing (docs only); reflects the three flags shipped in Task 5.

- [ ] **Step 1: Add a feature bullet to `apps/ccusage/README.md`**

Edit `apps/ccusage/README.md`, right after the existing model-breakdown bullet (around line 177:
`- 📊 **Model Breakdown**: View per-model cost breakdown with `--breakdown` flag`):

```markdown
- 📊 **Model Breakdown**: View per-model cost breakdown with `--breakdown` flag
- 🔌 **Attribution Breakdown**: View per-plugin, per-skill, and active/background cost breakdown
  with `--by-plugin`, `--by-skill`, and `--by-source-type` flags (plugin/skill attribution is
  Claude Code-specific; other agents report as `unattributed`)
```

- [ ] **Step 2: Add a usage example to `docs/guide/cli-options.md`**

Edit `docs/guide/cli-options.md`, right after the existing `--breakdown` example block (around
lines 58-67):

```markdown
\`\`\`bash
# Show per-model breakdown
ccusage daily --breakdown

# Show per-plugin, per-skill, and active/background breakdowns
ccusage daily --by-plugin
ccusage daily --by-skill
ccusage daily --by-source-type
\`\`\`
```

(Read the exact surrounding fenced-code-block formatting first — this repo's markdown files use
plain triple-backtick fences with a `bash` language tag for CLI examples, matching the existing
`--breakdown` block; do not add the literal backslashes above, they are only escaping this plan
document's own markdown.)

- [ ] **Step 3: Add the three new flags to the flags reference table**

`docs/guide/cli-options.md`'s short-alias table (around lines 340-343) only lists flags that have
a short alias (e.g. `` `--breakdown` | `-b` | Per-model breakdown ``). The three new flags have no
short alias, so do not add them there. Instead, add a short paragraph near that table (or wherever
the page documents flag families) explaining the bucket semantics:

```markdown
`--by-plugin` and `--by-skill` group usage by the plugin or skill that was active for each
assistant turn (Claude Code only); entries without attribution, or from other agents, appear
under `unattributed`. `--by-source-type` groups usage into `active` (main thread) and
`background` (sidechain/subagent) buckets.
```

- [ ] **Step 4: Verify docs build (if applicable)**

Run: `just --list | grep -i docs` to check for a docs build/lint recipe, and run it if one exists
(e.g. a VitePress dev-build or markdown-lint recipe) to confirm no broken links or lint failures
were introduced.

- [ ] **Step 5: Commit**

```bash
git add apps/ccusage/README.md docs/guide/cli-options.md
git commit -m "docs: document --by-plugin, --by-skill, --by-source-type flags"
```

---

## Self-Review

**Spec coverage:**
- Three new fields on `UsageEntry` (`attributionPlugin`/`attributionSkill` JSONL mapping): Task 1. ✓
- `is_sidechain` reused as-is for source-type: Task 1 (no new field), Tasks 3-4 (grouping logic). ✓
- Three new breakdown structs mirroring `ModelBreakdown`: Task 1. ✓
- Three new CLI flags on `SharedArgs`, independently composable with `--breakdown`/`--by-agent`/`--json`: Task 5 (flags), Task 6/8 (table gating proves composability with `--breakdown` since both gates coexist in the same loop), JSON is always-on so composes trivially. ✓
- Claude adapter whole-file path threading: Task 2 (verified via tests; no production change needed since `LoadedEntry.data: UsageEntry` already carries the fields). ✓
- Claude adapter streaming path threading: Task 3. ✓
- Sidechain-replay dedup untouched, fields ride along with the winning entry: Task 2 Step 6 (lib.rs test), Task 3 Step 3 comment (daily.rs dedup does whole-struct replacement, no logic touched). ✓
- Cross-agent `AllRow`/`AllAccumulator` extension: Task 7. ✓
- Cross-agent report rendering, non-claude defaults to `unattributed`/`active`: Task 8 (including an explicit test for this). ✓
- CLI wiring (`SharedArgs`, parser match arm, help text): Task 5. ✓
- Docs: Task 9. ✓
- Regression test that `--by-source-type` partitions the already-deduped list without double-counting: Task 2 Step 6 (dedup keeps one entry, source-type attribution reflects the winner) and Task 4 Step 2 (accumulator test asserts `active`/`background` totals split correctly from distinct entries). ✓
- Each flag works independently and in combination with `--breakdown`/`--by-agent`/`--json`: table gating in Tasks 6 and 8 places all four `if shared.X` checks side by side in the same loop body, proving independence; JSON is unconditional so it always "combines." ✓

**Placeholder scan:** No "TBD"/"similar to Task N"/"add appropriate handling" phrases were used;
every code step includes literal Rust/JSON/Markdown. Two steps (Task 6 Step 1's `SimpleTable`
test API, Task 8 Step 5's exact indent/signature of `push_model_breakdown_rows`) explicitly
instruct the implementer to read the real, currently-uninspected API/body first and adjust the
given code to match — these are flagged inline as verification steps, not left as vague
instructions, and the plan gives a concrete best-guess implementation to adjust rather than an
empty placeholder.

**Type consistency:** `PluginBreakdown { plugin_name, ... }`, `SkillBreakdown { skill_name, ... }`,
`SourceTypeBreakdown { source_type, ... }` (Task 1) are used with these exact field names in every
later task (Tasks 3, 4, 6, 7, 8). `UsageSummary.plugin_breakdowns` / `.skill_breakdowns` /
`.source_type_breakdowns` (Task 1) match the names used in Tasks 3, 4, 6. `AllRow.plugin_breakdowns`
/ `.skill_breakdowns` / `.source_type_breakdowns` (Task 7) match Task 8. `SharedArgs.by_plugin` /
`.by_skill` / `.by_source_type` (Task 5) match the flag checks in Tasks 6 and 8. Function names
`push_plugin_breakdown_rows`/`push_skill_breakdown_rows`/`push_source_type_breakdown_rows` are
distinct per-file (one set in `output.rs`, a differently-signed set in `report.rs`) and each is
only referenced within its own file's task — no cross-task name collision.
