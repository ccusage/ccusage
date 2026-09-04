# Usage Attribution Breakdown Design (Plugin / Skill / Source-Type)

## Context

Claude Code's session JSONL already carries three attribution fields on
assistant messages that ccusage does not currently surface:

- `message.model` — already used for the existing `--breakdown` (by-model)
  report; no changes needed there.
- `attributionPlugin` / `attributionSkill` — which plugin/skill was active for
  that assistant turn (e.g. `atlassian`, `aws`, `superpowers:brainstorming`).
  Not currently captured anywhere in the Rust codebase.
- `isSidechain` — `true` for subagent/background work, `false` for the
  main/active thread. Already captured on `UsageEntry`
  (`rust/crates/ccusage-core/src/types.rs:9-19`) but not exposed as a
  breakdown dimension.

This design adds three new breakdown dimensions — by plugin, by skill, and by
active-vs-background (source type) — following the same
accumulator/breakdown-struct pattern the existing `--breakdown` (model) and
`--by-agent` (agent source) flags already use. `attributionPlugin` and
`attributionSkill` are Claude-Code-specific concepts; entries from other
adapters (Codex, Gemini, Copilot, etc.) will have `None` for both and bucket
into an `unattributed` group.

The existing sidechain-replay dedup logic
(`rust/adapters/claude/src/lib.rs:126-140`, `daily.rs:429-503`) collapses
duplicate sidechain replays of the same message but does not exclude sidechain
entries from totals. The new active/background breakdown must partition the
already-deduped entry list by `is_sidechain` — it must not reintroduce a
second filtering pass, which could resurrect the double-counting bug that
logic works around.

## Decision

Add three new boolean CLI flags, each independently composable with existing
flags (`--breakdown`, `--by-agent`, `--json`, etc.):

- `--by-plugin` — groups usage by `attribution_plugin`, unknown → `unattributed`.
- `--by-skill` — groups usage by `attribution_skill`, unknown → `unattributed`.
- `--by-source-type` — groups usage by `is_sidechain` into `active` / `background`.

Each produces its own breakdown struct (`PluginBreakdown`, `SkillBreakdown`,
`SourceTypeBreakdown`), computed only when its flag is set, mirroring
`ModelBreakdown` (`rust/crates/ccusage-core/src/types.rs:98-111`) exactly:
name/key, token counts, cost, `missing_pricing`.

## Data Flow

1. **Capture**: `UsageEntry` (`ccusage-core/src/types.rs:9-19`) gains
   `attribution_plugin: Option<String>` and `attribution_skill: Option<String>`,
   serde-mapped from `attributionPlugin`/`attributionSkill` via the struct's
   existing camelCase rename. `is_sidechain` is unchanged (already present).

2. **Threading through the claude adapter's two parsing paths**: both
   `lib.rs`'s whole-file `LoadedEntry` construction and `daily.rs`'s streaming
   entry struct (`~120-190`, `~320-360`) get the two new fields added, matching
   how `is_sidechain` is already duplicated across both paths. Values pass
   through the existing sidechain-replay dedup untouched — dedup key stays
   `(message_id, session_id)`, attribution fields are carried along with
   whichever entry wins, not consulted by the dedup comparison itself.

3. **Aggregation**: three new accumulators in `ccusage-core/src/summary.rs`,
   structurally identical to `UsageAccumulator` (39-97) — group key is
   `attribution_plugin`, `attribution_skill`, or `is_sidechain` instead of
   `entry.model`. Each only runs when its flag is set, to avoid the extra pass
   when unused.

4. **Cross-agent (`--by-agent`/`all`) report**: `AllRow`
   (`rust/crates/ccusage-adapter-all/src/types.rs:7-22`) gains optional
   `plugin_breakdowns` / `skill_breakdowns` / `source_type_breakdowns` fields,
   populated the same way `model_breakdowns` is today via
   `merge_model_breakdowns`/`aggregate_model_breakdowns` (152-208) — new sibling
   functions per dimension, same merge shape.

5. **CLI wiring**: new bools on `SharedArgs`
   (`rust/crates/ccusage-cli/src/types.rs`, next to `breakdown` at line 45),
   parsed alongside `--breakdown` in
   `rust/crates/ccusage-cli-parser/src/parser.rs:747`, added to the flag list in
   the help text (~line 1010).

6. **Rendering**: single-agent reports render new breakdown rows in
   `ccusage-core/src/output.rs`, following `push_model_breakdown_rows` (~469)
   gated on the new flags (mirroring the `shared.breakdown` gate at ~246).
   Cross-agent `all`/`--by-agent` reports render via
   `ccusage-adapter-all/src/report.rs`, following the existing per-agent
   model-breakdown sub-row logic (~234-246).

## Required Changes

- `ccusage-core/src/types.rs`: add `attribution_plugin`, `attribution_skill` to
  `UsageEntry` and `LoadedEntry`; add `PluginBreakdown`, `SkillBreakdown`,
  `SourceTypeBreakdown` structs; add corresponding `Vec<...>` fields to
  `UsageSummary`.
- `ccusage-core/src/summary.rs`: three new accumulators + grouping logic,
  gated on their respective flags.
- `ccusage-core/src/output.rs`: three new row-rendering functions, gated on
  their respective flags, for table and JSON output.
- `rust/adapters/claude/src/lib.rs` and `daily.rs`: thread the two new fields
  through both parsing paths.
- `ccusage-cli/src/types.rs`: add `by_plugin`, `by_skill`, `by_source_type` to
  `SharedArgs`.
- `ccusage-cli-parser/src/parser.rs`: parse the three new flags; add to help
  text flag list.
- `ccusage-adapter-all/src/types.rs`: extend `AllRow` and `AllAccumulator`
  with the three new breakdown dimensions, mirroring the existing
  `model_breakdowns` merge/aggregate functions.
- `ccusage-adapter-all/src/report.rs`: render the three new breakdown
  dimensions in the cross-agent report.
- Test fixtures: add inline JSONL test strings with `attributionPlugin`,
  `attributionSkill`, and mixed `isSidechain` values, following the existing
  inline-fixture pattern in `lib.rs` and `daily.rs` test modules — no
  standalone fixture files exist for these fields today.
- Docs: update `apps/ccusage/README.md` (and any VitePress guide under
  `docs/guide/`) to document the three new flags, following `docs/AGENTS.md`
  conventions.

## Out of Scope

- Changing existing `--breakdown` (model) or `--by-agent` (agent source)
  behavior.
- Any change to the sidechain-replay dedup logic itself.
- Capturing new fields from adapters other than claude (Codex, Gemini,
  Copilot, etc. have no equivalent concept; their entries bucket as
  `unattributed`/`active` by default under the new dimensions).
- A combined/nested single-table view across all breakdown dimensions at once
  — each dimension renders as its own set of rows, consistent with how
  `--breakdown` and `--by-agent` already coexist as independent flags today.

## Validation

Tests will verify that:

- `attributionPlugin`/`attributionSkill` values round-trip from JSONL through
  `UsageEntry`/`LoadedEntry` into the correct breakdown bucket, including the
  `unattributed` bucket when absent.
- `--by-source-type` correctly partitions `active`/`background` using the
  already-deduped entry list, not raw pre-dedup entries — a regression test
  should assert totals match the non-flagged report's totals split across the
  two buckets (no double-counting).
- Each new flag works independently and in combination with `--breakdown`,
  `--by-agent`, and `--json`.
- Cross-agent `all` reports correctly show `unattributed`/`active` for
  non-claude agent rows.

## Next Steps

Per `CONTRIBUTING.md`, this is new public CLI surface (flags + JSON schema),
not a trivial fix — a GitHub issue describing this proposal should be opened
in `ryoppippi/ccusage` and get maintainer sign-off before a PR is opened.
