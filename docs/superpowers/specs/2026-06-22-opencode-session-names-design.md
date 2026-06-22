# Session names in the session report (OpenCode first)

- **Date:** 2026-06-22
- **Status:** Approved design, ready for implementation plan
- **Scope:** Native Rust CLI under `rust/crates/ccusage`

## Problem

The session report lists each session by its ID, token usage, and cost, but
never shows the user-facing session name. IDs like `ses_41af67070ffeTX3Kb6bDDf2IJ7`
are hard to recognize, so users cannot tell which session a row refers to.

An earlier read of the OpenCode adapter concluded "OpenCode logs have no session
name." That is only true of OpenCode *message* records — and those are all the
loader currently reads (`SELECT id, session_id, data FROM message` in
`adapter/opencode/loader.rs`). OpenCode stores session **titles separately**, so
the name is available; the loader simply never reads it.

### Verified OpenCode storage (real local data)

- **SQLite** (`~/.local/share/opencode/opencode.db`): a `session` table with
  **direct columns** `id` (PK) and `title` (`TEXT NOT NULL`), among others.
  Lookup is `SELECT id, title FROM session`. Join key is
  `message.session_id = session.id` (both use the `ses_…` form).
- **Legacy files** (`storage/session/<projectHash>/<sessionID>.json`): top-level
  `id` and `title` fields.

Example pairs observed: `ses_41af…` → `"Repository discovery and explanation"`,
`ses_1214b…` → `"Greeting"`.

## Goals

- Show the user-facing session name in the OpenCode session report (table + JSON).
- Build the supporting seam in the shared report layer so other agents can plug
  in their own name source later with minimal work.
- Keep every other agent and every other report mode (daily/weekly/monthly) and
  the native `ccusage session` (Claude Code) byte-for-byte unchanged.

## Non-goals

- Wiring up name sources for agents other than OpenCode (amp, codex, pi, claude,
  etc.). Those become follow-ups that only provide a `session_id → title` map.
- Changing how sessions are grouped, sorted, or filtered.
- Surfacing any OpenCode session field other than the title.

## Decisions (resolved during brainstorming)

- **Display:** add a separate **Name** column; keep the existing Session ID
  column. JSON gains a `sessionName` field. IDs stay visible for scripting and
  disambiguation.
- **Scope:** implement the OpenCode title source now, but put the reusable seam
  (column + JSON field + enrichment helper) in the shared layer.
- **Defaults:** column label `Name`; JSON key `sessionName`.

## Design

### 1. Data source (OpenCode-specific)

Add `load_session_names(&SharedArgs) -> HashMap<String, String>` to
`adapter/opencode/loader.rs`, mirroring the existing message fallback chain over
`paths()`:

- **SQLite primary:** open the same DB resolved by `db_path()`, run
  `SELECT id, title FROM session`, insert each `(id, title)` pair. A missing
  `session` table or a failed open returns an empty map and logs via the existing
  `debug_log` pattern — never an error.
- **Legacy fallback:** walk `storage/session/**/ses_*.json`, parse top-level `id`
  and `title`. Used to fill ids the DB pass did not supply.
- **Empty titles dropped:** a `""` title (the column is `NOT NULL` but may be
  empty) is treated as "no name" and omitted from the map.

The map is keyed by session id (`String`), matching `LoadedEntry.session_id`.

### 2. Generic seam (shared layer, agent-agnostic)

- **`UsageSummary.session_name: Option<String>`** in `types.rs`, defaulting to
  `None`. Update the two test fixture constructors that build `UsageSummary`
  literally (`adapter/opencode/report.rs` snapshot row, and any other literal
  site). No `LoadedEntry` changes — its 41 construction sites stay untouched.
- **Table** (`print_usage_table` in `output.rs`): add a conditional **`Name`**
  column immediately after the first column, gated on
  `rows.iter().any(|r| r.session_name.is_some())`, exactly mirroring the existing
  `include_last_activity` handling (header/align push + per-row value push +
  totals-row blank cell). The column appears only when at least one row has a
  name, so daily/weekly/monthly and other agents render identically to today.
  Rows without a name show a blank cell.
- **JSON:**
  - `agent_summary_json` (re-exported as `opencode::n`, shared by all agents):
    add `"sessionName"` **only inside the `include_session_metadata` branch**,
    which is session-only. Daily/weekly/monthly output is unchanged. Emit `null`
    when absent (consistent with the sibling `lastActivity`/`projectPath` keys in
    that branch).
  - `session_summary_json` (native `ccusage session`): add `"sessionName"` when
    present.
- **`apply_session_names(rows: &mut [UsageSummary], names: &HashMap<String, String>)`**
  in the shared layer: for each row, if `session_id` matches a map entry, set
  `session_name`. A no-op for rows without a `session_id` (daily/weekly/monthly).

### 3. OpenCode wiring (only producer now)

In `adapter/opencode/mod.rs::run`:

- After loading entries, call `loader::load_session_names(&shared)` once.
- **Table path:** `summarize_entries` → `apply_session_names(&mut rows, &names)`
  → `print_usage_table`.
- **JSON path:** thread the names map into `report_json` so it applies
  `apply_session_names` to the session-kind rows before serializing (or summarize
  + enrich in `run` and pass rows to a serialize step). Names only affect the
  `Session` kind; for other kinds the map matches nothing and is a no-op.

Other agents call the shared helpers with no map → `session_name` stays `None` →
unchanged behavior. The native `ccusage session` (Claude Code) has no title
source, so the column never appears there.

## Edge cases

- Session has messages but no title row → name stays `None`; table cell blank.
- Empty-string title → dropped, treated as no name.
- DB without a `session` table (older installs) → empty map, no error.
- Legacy-only install (no DB) → titles read from `storage/session` files.
- Duplicate ids across DB and legacy files → DB wins (matches existing
  message-dedup precedence).

## Testing

- **`load_session_names`:** fixture DB with a `session` table populated via the
  test helper pattern in `loader.rs`, plus a legacy
  `storage/session/<hash>/ses_*.json` fixture. Assert id→title mapping, empty
  title dropped, missing-table → empty map.
- **`apply_session_names`:** matching id sets the name; non-matching id leaves
  `None`; rows without `session_id` untouched.
- **Snapshots:** update the OpenCode report JSON snapshot to include
  `sessionName` under session metadata; add a session-report table snapshot that
  shows the Name column with a mix of named and unnamed rows. Verify
  daily/weekly/monthly snapshots are unchanged.

## Documentation

Per the repo cross-cutting flow, after implementation audit impact with the
`docs` skill and update:

- root `README.md` and `apps/ccusage/README.md` (session report + `sessionName`),
- the OpenCode docs guide under `docs/guide/`,
- any related cross-links / VitePress nav if a new option or column is documented.

No new user-facing flags are introduced (the column is automatic), so doc changes
are limited to describing the new column and JSON field.

## Affected files (anticipated)

- `rust/crates/ccusage/src/types.rs` — `UsageSummary.session_name`
- `rust/crates/ccusage/src/output.rs` — Name column, `session_summary_json`,
  `apply_session_names` (or a nearby shared module)
- `rust/crates/ccusage/src/adapter/opencode/report.rs` — `sessionName` in
  `agent_summary_json`; thread names into `report_json`
- `rust/crates/ccusage/src/adapter/opencode/loader.rs` — `load_session_names`
- `rust/crates/ccusage/src/adapter/opencode/mod.rs` — build + apply the map
- snapshots under `rust/crates/ccusage/src/adapter/opencode/snapshots/`
- docs: `README.md`, `apps/ccusage/README.md`, `docs/guide/*`
