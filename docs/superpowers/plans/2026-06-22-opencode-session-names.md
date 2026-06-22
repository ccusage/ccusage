# OpenCode Session Names Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show the user-facing session name in the OpenCode session report (table + JSON), via a reusable seam in the shared report layer that other agents can adopt later.

**Architecture:** Add an optional `session_name` to the shared `UsageSummary`, a generic `apply_session_names` helper, a conditional "Name" table column, and a `sessionName` JSON key — all gated so they appear only when a name exists. OpenCode is the only producer now: a new `load_session_names` loader reads titles from the `session` SQLite table (and legacy `storage/session` JSON files) and joins them by session id.

**Tech Stack:** Rust (`rust/crates/ccusage`), `sqlite` crate (read-only), `serde_json`, `insta` snapshots, `ccusage_test_support::fs_fixture`.

## Global Constraints

- Production CLI is Rust-first under `rust/crates/ccusage`; all runtime behavior goes there.
- Keep modules small and `pub(crate)` surfaces narrow; prefer fixture-backed parser/loader tests.
- Use `logger.rs` / `debug_log` (already imported in the loader), never `println!`/`console.log`, for diagnostics.
- This crate uses let-chains (`if let ... && let ... && cond`) — match that style.
- Run cargo checks through `just` recipes where possible. After code changes run `just fmt`; run `just test` / `just typecheck` for behavior changes.
- No new user-facing CLI flag: the Name column appears automatically whenever names exist.
- Every existing report mode (daily/weekly/monthly), every other agent, and the native `ccusage session` (Claude Code) must remain byte-for-byte unchanged.
- US English in all repo-facing text and docs.

## Verified facts (from real local OpenCode data)

- SQLite `session` table has direct columns `id` (PK) and `title` (`TEXT NOT NULL`). Query: `SELECT id, title FROM session`.
- Join key: `message.session_id == session.id` (both `ses_…`).
- Legacy file `storage/session/<projectHash>/<sessionID>.json` has top-level `id` and `title`.
- `LoadedEntry` is constructed in ~41 places — so the name rides on `UsageSummary`, NOT `LoadedEntry`.

## File Structure

- `rust/crates/ccusage/src/types.rs` — add `session_name` field to `UsageSummary`.
- `rust/crates/ccusage/src/summary.rs` — add `apply_session_names`; update literal `UsageSummary` builders + test fixture.
- `rust/crates/ccusage/src/main.rs` — re-export `apply_session_names`.
- `rust/crates/ccusage/src/output.rs` — `Name` column in `print_usage_table` (via new `usage_table_columns` helper), `sessionName` in `session_summary_json`, update literal builders/fixtures.
- `rust/crates/ccusage/src/adapter/opencode/report.rs` — `sessionName` in `agent_summary_json`; thread names into `report_json`; update snapshot test.
- `rust/crates/ccusage/src/adapter/opencode/loader.rs` — `load_session_names` (DB + legacy) + tests.
- `rust/crates/ccusage/src/adapter/opencode/mod.rs` — build + apply the names map.
- Other literal `UsageSummary` sites needing `session_name: None`: `adapter/all/report.rs`, `adapter/all/loader.rs` (test), `adapter/claude/daily.rs`, `adapter/qwen/mod.rs` (test).
- Docs: `README.md`, `apps/ccusage/README.md`, `docs/guide/*`.

---

### Task 1: Add `session_name` to `UsageSummary` and the `apply_session_names` helper

**Files:**
- Modify: `rust/crates/ccusage/src/types.rs:184` (struct field)
- Modify: `rust/crates/ccusage/src/summary.rs` (helper + literal builders at `:92`, `:200`, test fixture `:708`)
- Modify: `rust/crates/ccusage/src/main.rs:45-48` (re-export)
- Modify literal builders (add `session_name: None`): `output.rs:553`, `output.rs:733`, `adapter/opencode/report.rs:221`, `adapter/all/report.rs:195`, `adapter/all/loader.rs:647`, `adapter/claude/daily.rs:488`, `adapter/qwen/mod.rs:151`
- Test: `rust/crates/ccusage/src/summary.rs` (tests module)

**Interfaces:**
- Produces: `UsageSummary.session_name: Option<String>`; `pub(crate) fn apply_session_names(rows: &mut [UsageSummary], names: &std::collections::HashMap<String, String>)`

- [ ] **Step 1: Write the failing test** in the `summary.rs` `#[cfg(test)] mod tests` block (it already has `summary_row` + `SummaryFixture`):

```rust
#[test]
fn apply_session_names_sets_only_matching_rows() {
    let mut named = summary_row(SummaryFixture {
        date: Some("2026-01-02"),
        model: "m",
        cost: 1.0,
        input_tokens: 1,
    });
    named.session_id = Some("ses_a".to_string());
    let mut unnamed = summary_row(SummaryFixture {
        date: Some("2026-01-02"),
        model: "m",
        cost: 1.0,
        input_tokens: 1,
    });
    unnamed.session_id = Some("ses_b".to_string());
    let mut no_id = summary_row(SummaryFixture {
        date: Some("2026-01-02"),
        model: "m",
        cost: 1.0,
        input_tokens: 1,
    });
    no_id.session_id = None;

    let mut rows = vec![named, unnamed, no_id];
    let mut names = std::collections::HashMap::new();
    names.insert("ses_a".to_string(), "Greeting".to_string());
    apply_session_names(&mut rows, &names);

    assert_eq!(rows[0].session_name.as_deref(), Some("Greeting"));
    assert_eq!(rows[1].session_name, None);
    assert_eq!(rows[2].session_name, None);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `just test` (or `cargo test -p ccusage apply_session_names_sets_only_matching_rows`)
Expected: FAIL — `apply_session_names` not found and `session_name` field missing.

- [ ] **Step 3: Add the struct field** in `types.rs` immediately after `session_id` (line 184), matching the sibling attribute:

```rust
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) session_name: Option<String>,
```

- [ ] **Step 4: Add `session_name: None` to every literal `UsageSummary { ... }` builder.** Place it right after the `session_id:` line in each of:
  - `summary.rs:92` (`UsageAccumulator::into_summary`)
  - `summary.rs:200` (`aggregate_summaries`)
  - `summary.rs:708` (`summary_row` test fixture)
  - `output.rs:553` (`totals_json` test)
  - `output.rs:733` (`snapshot_summary` test fixture)
  - `adapter/opencode/report.rs:221` (`snapshot_row` test fixture)
  - `adapter/all/report.rs:195`
  - `adapter/all/loader.rs:647` (test)
  - `adapter/claude/daily.rs:488`
  - `adapter/qwen/mod.rs:151` (test)

Each edit looks like:

```rust
            session_id: ...,
            session_name: None,
```

Note: `SessionAccumulator::into_summary` (`summary.rs:149`) builds via `self.usage.into_summary()` (the `UsageAccumulator` builder edited above), so no separate field line is needed there — it inherits `None`.

- [ ] **Step 5: Add the helper** in `summary.rs` (top-level, near the other summary functions, before the tests module):

```rust
pub(crate) fn apply_session_names(
    rows: &mut [UsageSummary],
    names: &std::collections::HashMap<String, String>,
) {
    for row in rows.iter_mut() {
        if let Some(id) = row.session_id.as_deref()
            && let Some(name) = names.get(id)
        {
            row.session_name = Some(name.clone());
        }
    }
}
```

- [ ] **Step 6: Re-export the helper** in `main.rs` — add `apply_session_names` to the `pub(crate) use summary::{...}` list (lines 45-48):

```rust
pub(crate) use summary::{
    BucketKind, SessionAccumulator, apply_session_names, filter_and_sort_summaries,
    sort_summaries, summarize_by_key, summarize_summaries_by_bucket, week_start,
};
```

- [ ] **Step 7: Run tests + format**

Run: `just fmt && just test`
Expected: PASS — the new test passes; all existing tests still compile/pass (no snapshot changes yet, since `None` is not serialized).

- [ ] **Step 8: Commit**

```bash
git add rust/crates/ccusage/src
git commit -m "feat(session): add session_name field and apply_session_names helper"
```

---

### Task 2: Serialize `sessionName` in the JSON outputs

**Files:**
- Modify: `rust/crates/ccusage/src/output.rs:58-77` (`session_summary_json`)
- Modify: `rust/crates/ccusage/src/adapter/opencode/report.rs:30-72` (`agent_summary_json`) and its snapshot test (`:176-197`)
- Modify snapshot: `rust/crates/ccusage/src/adapter/opencode/snapshots/ccusage__adapter__opencode__report__tests__snapshots_agent_summary_json_period_keys_and_session_metadata.snap`
- Test: `output.rs` tests, `adapter/opencode/report.rs` tests

**Interfaces:**
- Consumes: `UsageSummary.session_name` (Task 1)
- Produces: JSON key `"sessionName"` present only when `session_name.is_some()`, in both `session_summary_json` (native `ccusage session`) and `agent_summary_json` (all agents' session reports). Not gated on `include_session_metadata`, so other report kinds are unaffected because their rows never carry a name.

- [ ] **Step 1: Write the failing tests.**

In `output.rs` tests module (uses existing `snapshot_summary` helper):

```rust
#[test]
fn session_summary_json_includes_session_name_when_present() {
    let mut row = snapshot_summary("2026-01-02", None, None);
    row.session_id = Some("ses_a".to_string());
    row.session_name = Some("Greeting".to_string());
    assert_eq!(
        session_summary_json(&row).get("sessionName").and_then(Value::as_str),
        Some("Greeting"),
    );

    let plain = snapshot_summary("2026-01-02", None, None);
    assert!(session_summary_json(&plain).get("sessionName").is_none());
}
```

In `adapter/opencode/report.rs` tests module (uses existing `snapshot_row` helper):

```rust
#[test]
fn agent_summary_json_includes_session_name_when_present() {
    let mut row = snapshot_row();
    row.session_name = Some("Greeting".to_string());
    let value = agent_summary_json(&row, AgentReportKind::Session, true);
    assert_eq!(value.get("sessionName").and_then(|v| v.as_str()), Some("Greeting"));

    let plain = snapshot_row();
    assert!(
        agent_summary_json(&plain, AgentReportKind::Session, true)
            .get("sessionName")
            .is_none()
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ccusage session_name`
Expected: FAIL — `sessionName` key missing.

- [ ] **Step 3: Add the insert to `session_summary_json`** (`output.rs`), right after the existing `credits` insert (before `value` is returned at line 76):

```rust
    if let (Some(obj), Some(credits)) = (value.as_object_mut(), row.credits) {
        obj.insert("credits".to_string(), json!(credits));
    }
    if let (Some(obj), Some(name)) = (value.as_object_mut(), row.session_name.as_ref()) {
        obj.insert("sessionName".to_string(), json!(name));
    }
    value
}
```

- [ ] **Step 4: Add the insert to `agent_summary_json`** (`adapter/opencode/report.rs`), right after the `messageCount` insert (around line 50), before the `include_session_metadata` block:

```rust
    if let (Some(obj), Some(message_count)) = (value.as_object_mut(), row.message_count) {
        obj.insert("messageCount".to_string(), json!(message_count));
    }
    if let (Some(obj), Some(name)) = (value.as_object_mut(), row.session_name.as_ref()) {
        obj.insert("sessionName".to_string(), json!(name));
    }
```

- [ ] **Step 5: Update the snapshot test** in `adapter/opencode/report.rs` so the `session` variant carries a name (leave `daily`/`weekly`/`monthly` untouched). In `snapshots_agent_summary_json_period_keys_and_session_metadata`, after the existing `session.month = None;` line add:

```rust
        session.session_name = Some("Repository discovery and explanation".to_string());
```

- [ ] **Step 6: Run + accept the snapshot**

Run: `cargo test -p ccusage` then `cargo insta review` (or `cargo insta accept`)
Expected: the unit tests PASS; the snapshot diff adds `"sessionName": "Repository discovery and explanation"` ONLY under the `session` and `sessionReport` entries. Confirm `daily`/`weekly`/`monthly` are unchanged before accepting.

- [ ] **Step 7: Commit**

```bash
git add rust/crates/ccusage/src
git commit -m "feat(session): serialize sessionName in session JSON outputs"
```

---

### Task 3: Add the conditional "Name" column to the usage table

**Files:**
- Modify: `rust/crates/ccusage/src/output.rs` — new `usage_table_columns` helper; `print_usage_table` (`:145`) header/align/row construction; `push_breakdown_rows` (`:405`); totals row (`:321`)
- Test: `output.rs` tests module

**Interfaces:**
- Consumes: `UsageSummary.session_name` (Task 1)
- Produces: a `Name` column inserted at index 1 (right after the first column) whenever any row has a name; `fn usage_table_columns<'a>(first_column: &'a str, compact: bool, no_cost: bool, include_last_activity: bool, include_session_name: bool) -> (Vec<&'a str>, Vec<Align>)`

- [ ] **Step 1: Write the failing tests** in `output.rs` tests module:

```rust
#[test]
fn usage_table_columns_inserts_name_after_first_when_requested() {
    let (headers, aligns) =
        usage_table_columns("Session", false, false, true, true);
    assert_eq!(headers[0], "Session");
    assert_eq!(headers[1], "Name");
    assert_eq!(*headers.last().unwrap(), "Last Activity");
    assert_eq!(headers.len(), aligns.len());
    assert!(matches!(aligns[1], Align::Left));
}

#[test]
fn usage_table_columns_omits_name_by_default() {
    let (headers, aligns) =
        usage_table_columns("Date", false, false, false, false);
    assert!(!headers.contains(&"Name"));
    assert_eq!(headers.len(), aligns.len());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ccusage usage_table_columns`
Expected: FAIL — `usage_table_columns` not found.

- [ ] **Step 3: Add the `usage_table_columns` helper** in `output.rs` (above `print_usage_table`):

```rust
fn usage_table_columns<'a>(
    first_column: &'a str,
    compact: bool,
    no_cost: bool,
    include_last_activity: bool,
    include_session_name: bool,
) -> (Vec<&'a str>, Vec<Align>) {
    let mut headers = if compact {
        vec![first_column, "Models", "Input", "Output", "Cost (USD)"]
    } else {
        vec![
            first_column,
            "Models",
            "Input",
            "Output",
            "Cache Create",
            "Cache Read",
            "Total Tokens",
            "Cost (USD)",
        ]
    };
    let mut aligns = if compact {
        vec![Align::Left, Align::Left, Align::Right, Align::Right, Align::Right]
    } else {
        vec![
            Align::Left,
            Align::Left,
            Align::Right,
            Align::Right,
            Align::Right,
            Align::Right,
            Align::Right,
            Align::Right,
        ]
    };
    if no_cost {
        headers.pop();
        aligns.pop();
    }
    if include_session_name {
        headers.insert(1, "Name");
        aligns.insert(1, Align::Left);
    }
    if include_last_activity {
        headers.push("Last Activity");
        aligns.push(Align::Left);
    }
    (headers, aligns)
}
```

- [ ] **Step 4: Replace the inline header/align construction in `print_usage_table`.** Delete the current `let mut headers = ...`, `let mut aligns = ...`, the `if shared.no_cost { headers.pop(); aligns.pop(); }`, and the `if include_last_activity { headers.push(...); aligns.push(...); }` blocks (lines ~167-208) and replace with:

```rust
    let include_last_activity = rows.iter().any(|row| row.last_activity.is_some());
    let include_session_name = rows.iter().any(|row| row.session_name.is_some());
    print_box_title(title, shared);
    let (headers, aligns) = usage_table_columns(
        first_column,
        compact,
        shared.no_cost,
        include_last_activity,
        include_session_name,
    );
```

- [ ] **Step 5: Insert the per-row name cell** in `print_usage_table`'s row loop. After the existing `if shared.no_cost { values.pop(); }` block and before the `if include_last_activity { values.push(...); }` block (lines ~258-265), add:

```rust
        if shared.no_cost {
            values.pop();
        }
        if include_session_name {
            values.insert(1, row.session_name.clone().unwrap_or_default());
        }
        if include_last_activity {
            values.push(truncate_rfc3339_to_date(
                row.last_activity.as_deref().unwrap_or_default(),
            ));
        }
        table.push(values);
        if shared.breakdown {
            push_breakdown_rows(&mut table, row, compact, include_last_activity, include_session_name, shared);
        }
```

- [ ] **Step 6: Insert the totals-row blank cell.** In the totals section, after `if shared.no_cost { total_row.pop(); }` and before `if include_last_activity { total_row.push(String::new()); }` (lines ~318-323), add:

```rust
    if shared.no_cost {
        total_row.pop();
    }
    if include_session_name {
        total_row.insert(1, String::new());
    }
    if include_last_activity {
        total_row.push(String::new());
    }
```

- [ ] **Step 7: Thread the flag into `push_breakdown_rows`.** Change its signature and add the blank cell:

```rust
fn push_breakdown_rows(
    table: &mut SimpleTable,
    row: &UsageSummary,
    compact: bool,
    include_last_activity: bool,
    include_session_name: bool,
    shared: &SharedArgs,
) {
```

and inside, after `if shared.no_cost { values.pop(); }` and before `if include_last_activity { values.push(String::new()); }` (lines ~453-458):

```rust
        if shared.no_cost {
            values.pop();
        }
        if include_session_name {
            values.insert(1, String::new());
        }
        if include_last_activity {
            values.push(String::new());
        }
        table.push(values);
```

- [ ] **Step 8: Run tests + format**

Run: `just fmt && cargo test -p ccusage`
Expected: PASS — new column tests pass; existing table-related tests unchanged (no rows carry `session_name` in those tests).

- [ ] **Step 9: Commit**

```bash
git add rust/crates/ccusage/src/output.rs
git commit -m "feat(session): add conditional Name column to the usage table"
```

---

### Task 4: Load OpenCode session names (DB + legacy files)

**Files:**
- Modify: `rust/crates/ccusage/src/adapter/opencode/loader.rs`
- Test: same file's `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: existing `paths()`, `db_path()`, `collect_files_with_extension`, `debug_log`, `SharedArgs`
- Produces: `pub(crate) fn load_session_names(shared: &SharedArgs) -> std::collections::HashMap<String, String>` and the directory-scoped `fn load_session_names_from_directory(opencode_dir: &Path, names: &mut HashMap<String, String>, shared: &SharedArgs)` used by tests. Map is keyed by session id, DB titles take precedence over legacy-file titles, empty titles are dropped.

- [ ] **Step 1: Write the failing test** in the loader `tests` module (it already imports `fs_fixture` and `SharedArgs`). Add a `create_db_session` helper and the test:

```rust
fn create_db_session(path: &Path, id: &str, title: &str) {
    let db = sqlite::open(path).unwrap();
    db.execute("CREATE TABLE IF NOT EXISTS session (id TEXT, title TEXT)")
        .unwrap();
    let mut statement = db
        .prepare("INSERT INTO session (id, title) VALUES (?1, ?2)")
        .unwrap();
    statement.bind((1, id)).unwrap();
    statement.bind((2, title)).unwrap();
    statement.next().unwrap();
}

#[test]
fn loads_session_names_db_wins_over_legacy_and_drops_empty() {
    let fixture = fs_fixture!({
        "storage/session/proj/ses_legacy.json": r#"{"id":"ses_legacy","title":"Legacy session"}"#,
        "storage/session/proj/ses_a.json": r#"{"id":"ses_a","title":"Stale file title"}"#,
    });
    create_db_session(&fixture.path("opencode.db"), "ses_a", "Greeting");
    create_db_session(&fixture.path("opencode.db"), "ses_empty", "");

    let mut names = std::collections::HashMap::new();
    super::load_session_names_from_directory(
        fixture.root(),
        &mut names,
        &SharedArgs::default(),
    );

    assert_eq!(names.get("ses_a").map(String::as_str), Some("Greeting"));
    assert_eq!(names.get("ses_legacy").map(String::as_str), Some("Legacy session"));
    assert_eq!(names.get("ses_empty"), None);
}
```

Also add `use super::load_session_names_from_directory;` is unnecessary if you call via `super::` as above — keep the explicit path.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ccusage loads_session_names`
Expected: FAIL — `load_session_names_from_directory` not found.

- [ ] **Step 3: Add the `HashMap` import** at the top of `loader.rs` (extend the existing `std::{...}` use):

```rust
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};
```

- [ ] **Step 4: Add the session-info struct** near the top of `loader.rs` (after the imports):

```rust
#[derive(serde::Deserialize)]
struct OpenCodeSessionInfo {
    id: Option<String>,
    title: Option<String>,
}
```

- [ ] **Step 5: Add the loader functions** in `loader.rs`:

```rust
pub(crate) fn load_session_names(shared: &SharedArgs) -> HashMap<String, String> {
    let mut names = HashMap::new();
    let Ok(dirs) = paths() else {
        return names;
    };
    for dir in dirs {
        load_session_names_from_directory(&dir, &mut names, shared);
    }
    names
}

fn load_session_names_from_directory(
    opencode_dir: &Path,
    names: &mut HashMap<String, String>,
    shared: &SharedArgs,
) {
    if let Some(db_path) = db_path(opencode_dir) {
        load_session_names_from_database(&db_path, names, shared);
    }

    let session_dir = opencode_dir.join("storage").join("session");
    let mut files = Vec::new();
    collect_files_with_extension(&session_dir, "json", &mut files);
    for file in files {
        if let Ok(content) = fs::read(&file)
            && let Ok(info) = serde_json::from_slice::<OpenCodeSessionInfo>(&content)
            && let (Some(id), Some(title)) = (info.id, info.title)
            && !title.is_empty()
        {
            names.entry(id).or_insert(title);
        }
    }
}

fn load_session_names_from_database(
    db_path: &Path,
    names: &mut HashMap<String, String>,
    shared: &SharedArgs,
) {
    let Ok(connection) =
        sqlite::Connection::open_with_flags(db_path, sqlite::OpenFlags::new().with_read_only())
    else {
        debug_log(
            shared,
            format!("Failed to open OpenCode database: {}", db_path.display()),
        );
        return;
    };
    let Ok(mut statement) = connection.prepare("SELECT id, title FROM session") else {
        debug_log(
            shared,
            format!(
                "OpenCode database has no session table: {}",
                db_path.display()
            ),
        );
        return;
    };
    while let Ok(sqlite::State::Row) = statement.next() {
        let Ok(id) = statement.read::<String, _>(0) else {
            continue;
        };
        let Ok(title) = statement.read::<String, _>(1) else {
            continue;
        };
        if !title.is_empty() {
            names.insert(id, title);
        }
    }
}
```

- [ ] **Step 6: Run test + format**

Run: `just fmt && cargo test -p ccusage loads_session_names`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add rust/crates/ccusage/src/adapter/opencode/loader.rs
git commit -m "feat(opencode): load session titles from db and legacy files"
```

---

### Task 5: Wire session names into the OpenCode report

**Files:**
- Modify: `rust/crates/ccusage/src/adapter/opencode/report.rs` (`report_json` signature)
- Modify: `rust/crates/ccusage/src/adapter/opencode/mod.rs` (`run`)

**Interfaces:**
- Consumes: `loader::load_session_names` (Task 4), `apply_session_names` (Task 1), `report_json` (modified here)
- Produces: OpenCode session table + JSON populated with names. `report_json(entries, kind, order, names: &HashMap<String, String>)`. Names are loaded only for the `Session` kind.

- [ ] **Step 1: Change `report_json`** in `adapter/opencode/report.rs`. Add the import and the parameter + enrichment:

At the top, extend the `use crate::{...}` to include `apply_session_names`, and add `use std::collections::HashMap;`. Then:

```rust
pub(crate) fn report_json(
    entries: &[LoadedEntry],
    kind: AgentReportKind,
    order: &SortOrder,
    names: &HashMap<String, String>,
) -> Result<Value> {
    let mut rows = summarize_entries(entries, kind)?;
    apply_session_names(&mut rows, names);
    sort_summaries(&mut rows, order, |row| summary_period(row));
    Ok(report_from_rows(&rows, kind))
}
```

- [ ] **Step 2: Update `run`** in `adapter/opencode/mod.rs`. Extend imports to bring in `AgentReportKind` and `apply_session_names`, then build and apply names:

```rust
use crate::{
    Result, apply_session_names, cli::AgentCommandArgs, cli::AgentReportKind,
    filter_loaded_entries_by_date, print_json_or_jq, print_usage_table, sort_summaries, wants_json,
};

pub(crate) fn run(args: AgentCommandArgs) -> Result<()> {
    let shared = args.shared;
    let mut entries = loader::load_entries(&shared)?;
    filter_loaded_entries_by_date(&mut entries, &shared);
    let names = if args.kind == AgentReportKind::Session {
        loader::load_session_names(&shared)
    } else {
        std::collections::HashMap::new()
    };
    if wants_json(&shared) {
        return print_json_or_jq(
            report_json(&entries, args.kind, &shared.order, &names)?,
            shared.jq.as_deref(),
            shared.no_cost,
        );
    }
    let mut rows = summarize_entries(&entries, args.kind)?;
    apply_session_names(&mut rows, &names);
    sort_summaries(&mut rows, &shared.order, |row| summary_period(row));
    print_usage_table(
        "OpenCode Token Usage Report",
        first_column(args.kind),
        &rows,
        &shared,
        false,
        None,
    )?;
    Ok(())
}
```

(Confirm `AgentReportKind` is the correct path; it is already used in `report.rs` via `crate::cli::AgentReportKind`. Verify `args.kind` derives `PartialEq`; if not, match on it instead: `matches!(args.kind, AgentReportKind::Session)`.)

- [ ] **Step 3: Build + run the full test suite**

Run: `just fmt && cargo test -p ccusage`
Expected: PASS — compiles with the new `report_json` arity; all tests green.

- [ ] **Step 4: Manual verification against real local OpenCode data** (the table path has no unit test because `SimpleTable::print` writes to stdout):

```bash
just build   # or: cargo build -p ccusage
# JSON: confirm sessionName appears for named sessions
./rust/target/debug/ccusage opencode --session --json | jq '.sessions[0]'
# Table: confirm a "Name" column appears after "Session"
./rust/target/debug/ccusage opencode --session | head -20
```

Expected: JSON rows include `"sessionName"` for sessions that have a title (e.g. "Repository discovery and explanation"); the table shows a `Name` column with titles, blank where a session has none. Daily/monthly/weekly views show no `Name` column.

- [ ] **Step 5: Verify responsive/table layout** per repo convention using the `cmux-debug` skill (table width, compact mode, breakdown rows with `--breakdown`).

- [ ] **Step 6: Commit**

```bash
git add rust/crates/ccusage/src/adapter/opencode
git commit -m "feat(opencode): show session names in the session report"
```

---

### Task 6: Documentation

**Files:**
- Modify: `README.md`, `apps/ccusage/README.md`, relevant `docs/guide/*` (OpenCode guide)

**Interfaces:**
- Consumes: the shipped behavior from Tasks 1-5.

- [ ] **Step 1: Audit documentation impact** using the `docs` skill to find every page that documents the OpenCode session report, session JSON fields, or example output.

- [ ] **Step 2: Update the OpenCode docs guide** to describe the new `Name` column in the session report and the `sessionName` JSON field, including a short note that the name comes from OpenCode's stored session title and is blank when unavailable. Update any example table/JSON snippets that show session output.

- [ ] **Step 3: Update `README.md` and `apps/ccusage/README.md`** where the session report or its JSON shape is shown, mirroring the guide. Add VitePress nav/cross-links only if a new page/section is introduced (none expected — this extends existing content).

- [ ] **Step 4: Verify docs build** if the repo provides a docs check (e.g. `just docs` / VitePress build) and proofread for US English.

- [ ] **Step 5: Commit**

```bash
git add README.md apps/ccusage/README.md docs
git commit -m "docs: document OpenCode session name column and sessionName field"
```

---

## Self-Review

**Spec coverage:**
- Data source (DB `session` table + legacy files, empty-title drop, missing-table graceful) → Task 4 ✓
- `UsageSummary.session_name` → Task 1 ✓
- Conditional `Name` table column mirroring `include_last_activity` → Task 3 ✓
- `sessionName` in `agent_summary_json` + `session_summary_json` → Task 2 ✓
- `apply_session_names` shared helper → Task 1 ✓
- OpenCode wiring (table + JSON paths, names only for Session kind) → Task 5 ✓
- Other agents / Claude `ccusage session` unchanged → guaranteed by `None` default + `.is_some()` gating, exercised by unchanged snapshots in Tasks 2-3 ✓
- Edge cases (no title, empty title, no DB, legacy-only, DB-vs-legacy precedence) → Task 4 test ✓
- Docs → Task 6 ✓

Refinement vs spec: the spec said `sessionName` would live "inside the `include_session_metadata` branch." Implementation instead gates on `session_name.is_some()` (independent of that flag) because OpenCode's `report_from_rows` passes `include_session_metadata = false`. This is strictly safer — daily/weekly/monthly rows never carry a name, so output is unchanged — and is the only way the name reaches OpenCode session JSON. Behavior matches the spec's intent.

**Placeholder scan:** No TBD/TODO; every code step shows complete code; the only non-code step is the docs audit (Task 6) which is inherently exploratory and bounded to named files.

**Type consistency:** `apply_session_names(&mut [UsageSummary], &HashMap<String, String>)`, `usage_table_columns(...) -> (Vec<&str>, Vec<Align>)`, `load_session_names(&SharedArgs) -> HashMap<String, String>`, `load_session_names_from_directory(&Path, &mut HashMap<String, String>, &SharedArgs)`, and `report_json(.., &HashMap<String, String>)` are used consistently across tasks. `push_breakdown_rows` gains the `include_session_name: bool` param at both its definition and its single call site (Task 3).
