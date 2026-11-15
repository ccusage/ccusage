use std::{collections::HashSet, path::Path};

use ccusage_adapter_common::{
    source::LoadContext,
    units::{UnitError, read_units_flat},
};
use ccusage_core::{Diagnostics, LoadedEntry, Result, Skip, SkipKind};

use super::{
    parser::{HermesEntry, read_session_row, to_loaded_entry},
    paths::hermes_state_db_paths,
};

pub(crate) fn load_entries_with_context(cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
    let pricing = cx
        .pricing
        .ok_or_else(|| crate::cli_error("Hermes needs pricing from the report pipeline"))?;
    let tz = Some(cx.window.timezone());
    let db_paths = hermes_state_db_paths()?;
    // Load each state database in parallel (a fresh read-only connection per DB),
    // then run the sequential session dedup over the original path order so the
    // surviving session matches the single-threaded read.
    let loaded = read_units_flat(
        &db_paths,
        cx.shared.single_thread,
        cx.diagnostics,
        |db_path| load_state_db_entries(db_path, cx.diagnostics),
    );
    let mut entries = Vec::new();
    let mut seen_sessions = HashSet::new();
    for entry in loaded {
        if !seen_sessions.insert(entry.session_id.clone()) {
            continue;
        }
        entries.push(to_loaded_entry(entry, tz, cx.shared.mode, pricing));
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

fn load_state_db_entries(
    db_path: &Path,
    diagnostics: &Diagnostics,
) -> std::result::Result<Vec<HermesEntry>, UnitError> {
    let connection =
        sqlite::Connection::open_with_flags(db_path, sqlite::OpenFlags::new().with_read_only())
            .map_err(|error| {
                let (kind, detail) = state_database_open_failure(error.code);
                UnitError::new(kind, detail)
            })?;
    let mut statement = connection
        .prepare(
            "
            SELECT
                id,
                model,
                billing_provider,
                started_at,
                message_count,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens,
                reasoning_tokens,
                estimated_cost_usd,
                actual_cost_usd
            FROM sessions
            WHERE model IS NOT NULL
                AND TRIM(model) != ''
        ",
        )
        .map_err(|error| {
            let (kind, detail) = sessions_query_failure(error.code);
            UnitError::new(kind, detail)
        })?;
    let mut entries = Vec::new();
    let mut row_number = 0_u32;
    loop {
        match statement.next() {
            Ok(sqlite::State::Row) => {
                row_number = row_number.saturating_add(1);
                match read_session_row(&statement) {
                    Ok(Some(entry)) => entries.push(entry),
                    Ok(None) => {}
                    Err(error) => diagnostics.record(Skip::record(
                        db_path,
                        row_number,
                        SkipKind::InvalidData,
                        error.detail(),
                    )),
                }
            }
            Ok(sqlite::State::Done) => break,
            Err(error) => {
                let (kind, detail) = sessions_step_failure(error.code);
                diagnostics.record(Skip::unit(db_path, kind, detail));
                break;
            }
        }
    }
    Ok(entries)
}

fn sessions_query_failure(code: Option<isize>) -> (SkipKind, &'static str) {
    match primary_sqlite_code(code) {
        Some(1) | Some(17) => (SkipKind::Invariant, "Hermes sessions schema is unavailable"),
        Some(11) | Some(26) => (
            SkipKind::InvalidData,
            "Hermes state database is corrupt or invalid",
        ),
        _ => (SkipKind::Unreadable, "Hermes sessions could not be read"),
    }
}

fn state_database_open_failure(code: Option<isize>) -> (SkipKind, &'static str) {
    match primary_sqlite_code(code) {
        Some(11) | Some(26) => (
            SkipKind::InvalidData,
            "Hermes state database is corrupt or invalid",
        ),
        _ => (SkipKind::Unreadable, "could not open Hermes state database"),
    }
}

fn sessions_step_failure(code: Option<isize>) -> (SkipKind, &'static str) {
    match primary_sqlite_code(code) {
        Some(1) | Some(17) => (
            SkipKind::Invariant,
            "Hermes sessions schema changed while reading; earlier rows were kept",
        ),
        Some(11) | Some(26) => (
            SkipKind::InvalidData,
            "Hermes state database is corrupt or invalid; earlier rows were kept",
        ),
        _ => (
            SkipKind::ReadInterrupted,
            "reading Hermes sessions stopped early; loaded rows were kept",
        ),
    }
}

fn primary_sqlite_code(code: Option<isize>) -> Option<isize> {
    code.map(|code| code & 0xff)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use ccusage_adapter_common::source::EntrySource;
    use ccusage_adapter_common::{parity::assert_focused_matches_unified, source::SourceOptions};
    use ccusage_core::{
        PricingMap, ReportWindow, SkipKind,
        cli::{AgentReportKind, SharedArgs},
    };
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    fn create_state_db(path: &Path) {
        let db = sqlite::open(path).unwrap();
        db.execute(
            "
                CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    source TEXT NOT NULL,
                    model TEXT,
                    started_at REAL NOT NULL,
                    message_count INTEGER DEFAULT 0,
                    input_tokens INTEGER DEFAULT 0,
                    output_tokens INTEGER DEFAULT 0,
                    cache_read_tokens INTEGER DEFAULT 0,
                    cache_write_tokens INTEGER DEFAULT 0,
                    reasoning_tokens INTEGER DEFAULT 0,
                    billing_provider TEXT,
                    estimated_cost_usd REAL,
                    actual_cost_usd REAL
                );
            ",
        )
        .unwrap();
    }

    #[test]
    fn loads_billable_hermes_sessions_from_state_db() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("state.db");
        create_state_db(&db_path);
        let db = sqlite::open(&db_path).unwrap();
        let mut statement = db
            .prepare(
                "
                    INSERT INTO sessions (
                        id, source, model, started_at, message_count,
                        input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens,
                        billing_provider, estimated_cost_usd, actual_cost_usd
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                ",
            )
            .unwrap();
        statement.bind((1, "session-1")).unwrap();
        statement.bind((2, "cli")).unwrap();
        statement.bind((3, "claude-sonnet-4-20250514")).unwrap();
        statement.bind((4, 1_750_000_000.25)).unwrap();
        statement.bind((5, 42_i64)).unwrap();
        statement.bind((6, 1200_i64)).unwrap();
        statement.bind((7, 300_i64)).unwrap();
        statement.bind((8, 50_i64)).unwrap();
        statement.bind((9, 20_i64)).unwrap();
        statement.bind((10, 10_i64)).unwrap();
        statement.bind((11, "anthropic")).unwrap();
        statement.bind((12, 0.12)).unwrap();
        statement.bind((13, 0.34)).unwrap();
        statement.next().unwrap();

        let pricing = PricingMap::load_embedded();
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let tz = crate::parse_tz(shared.timezone.as_deref());
        let diagnostics = Diagnostics::new("hermes");
        let entries = load_state_db_entries(&db_path, &diagnostics)
            .unwrap()
            .into_iter()
            .map(|entry| to_loaded_entry(entry, tz.as_ref(), shared.mode, &pricing))
            .collect::<Vec<_>>();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2025-06-15");
        assert_eq!(entries[0].session_id.as_ref(), "session-1");
        assert_eq!(
            entries[0].model.as_deref(),
            Some("claude-sonnet-4-20250514")
        );
        assert_eq!(entries[0].data.message.usage.input_tokens, 1200);
        assert_eq!(entries[0].data.message.usage.output_tokens, 300);
        assert_eq!(
            entries[0].data.message.usage.cache_creation_input_tokens,
            20
        );
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 50);
        assert_eq!(entries[0].extra_total_tokens, 10);
        assert_eq!(entries[0].message_count, Some(42));
        assert_eq!(entries[0].cost, 0.34);
    }

    #[test]
    fn classifies_state_database_query_failures_and_keeps_healthy_sessions() {
        let fixture = fs_fixture!({
            "good/state.db": "",
            "broken/state.db": "not a SQLite database",
            "schema/state.db": "",
        });
        let good_db = fixture.path("good/state.db");
        create_state_db(&good_db);
        sqlite::open(&good_db)
            .unwrap()
            .execute(
                "INSERT INTO sessions (id, source, model, started_at, input_tokens, output_tokens) VALUES ('session-a', 'cli', 'gpt-5', 1750000000, 100, 50)",
            )
            .unwrap();
        sqlite::open(fixture.path("schema/state.db"))
            .unwrap()
            .execute("CREATE TABLE unrelated (id TEXT)")
            .unwrap();
        let homes = format!(
            "{},{},{}",
            fixture.root().join("good").display(),
            fixture.root().join("broken").display(),
            fixture.root().join("schema").display()
        );
        let _cleanup = EnvVarGuard::set("HERMES_HOME", homes);
        let shared = SharedArgs::default();
        let pricing = PricingMap::load_embedded();
        let window = ReportWindow::from_shared(&shared);
        let diagnostics = Diagnostics::new("hermes");
        let cx = LoadContext {
            shared: &shared,
            kind: AgentReportKind::Daily,
            window: &window,
            pricing: Some(&pricing),
            options: SourceOptions::default(),
            diagnostics: &diagnostics,
        };

        let entries = crate::SOURCE.load_entries(&cx).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id.as_ref(), "session-a");
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        assert_eq!(diagnostics.skipped(SkipKind::Invariant), 1);
    }

    #[test]
    fn classifies_sessions_query_failures_by_sqlite_error_code() {
        assert_eq!(sessions_query_failure(Some(1)).0, SkipKind::Invariant);
        assert_eq!(sessions_query_failure(Some(17)).0, SkipKind::Invariant);
        assert_eq!(sessions_query_failure(Some(273)).0, SkipKind::Invariant);
        assert_eq!(sessions_query_failure(Some(11)).0, SkipKind::InvalidData);
        assert_eq!(sessions_query_failure(Some(26)).0, SkipKind::InvalidData);
        assert_eq!(sessions_query_failure(Some(267)).0, SkipKind::InvalidData);
        assert_eq!(sessions_query_failure(Some(282)).0, SkipKind::InvalidData);
        for code in [Some(5), Some(6), Some(10), None] {
            assert_eq!(sessions_query_failure(code).0, SkipKind::Unreadable);
        }
    }

    #[test]
    fn diagnoses_invalid_identity_or_timestamp_only_for_nonempty_usage_rows() {
        let fixture = fs_fixture!({"state.db": ""});
        let db_path = fixture.path("state.db");
        create_state_db(&db_path);
        sqlite::open(&db_path)
            .unwrap()
            .execute(
                "INSERT INTO sessions (id, source, model, started_at, input_tokens, output_tokens) VALUES ('before', 'cli', 'gpt-5', 1750000000, 100, 50), ('', 'cli', 'gpt-5', 1750000001, 100, 50), ('bad-time', 'cli', 'gpt-5', 'SECRET_CANARY', 100, 50), (NULL, 'cli', 'gpt-5', 'SECRET_CANARY', 0, 0), ('after', 'cli', 'gpt-5', 1750000002, 100, 50)",
            )
            .unwrap();
        let diagnostics = Diagnostics::new("hermes");

        let entries = load_state_db_entries(&db_path, &diagnostics).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].session_id, "before");
        assert_eq!(entries[1].session_id, "after");
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 2);
        let warnings = diagnostics
            .warnings(&SharedArgs {
                debug: true,
                ..Default::default()
            })
            .join("\n");
        assert!(warnings.contains("session identity"));
        assert!(warnings.contains("timestamp"));
        assert!(!warnings.contains("SECRET_CANARY"));
    }

    #[test]
    fn classifies_state_database_open_failures_by_sqlite_error_code() {
        for code in [Some(11), Some(26), Some(267), Some(282)] {
            assert_eq!(state_database_open_failure(code).0, SkipKind::InvalidData);
        }
        for code in [Some(5), Some(6), Some(10), Some(1), None] {
            assert_eq!(state_database_open_failure(code).0, SkipKind::Unreadable);
        }
    }

    #[test]
    fn classifies_sessions_step_failures_without_losing_interruption_semantics() {
        assert_eq!(sessions_step_failure(Some(1)).0, SkipKind::Invariant);
        assert_eq!(sessions_step_failure(Some(17)).0, SkipKind::Invariant);
        assert_eq!(sessions_step_failure(Some(273)).0, SkipKind::Invariant);
        assert_eq!(sessions_step_failure(Some(11)).0, SkipKind::InvalidData);
        assert_eq!(sessions_step_failure(Some(26)).0, SkipKind::InvalidData);
        assert_eq!(sessions_step_failure(Some(267)).0, SkipKind::InvalidData);
        assert_eq!(sessions_step_failure(Some(282)).0, SkipKind::InvalidData);
        for code in [Some(5), Some(6), Some(10), Some(261), None] {
            assert_eq!(sessions_step_failure(code).0, SkipKind::ReadInterrupted);
        }
    }

    #[test]
    fn focused_and_unified_reports_agree() {
        let fixture = fs_fixture!({"state.db": ""});
        let db_path = fixture.path("state.db");
        create_state_db(&db_path);
        sqlite::open(&db_path)
            .unwrap()
            .execute(
                "INSERT INTO sessions (id, source, model, started_at, input_tokens, output_tokens) VALUES ('session-a', 'cli', 'gpt-5', 1750000000, 100, 50)",
            )
            .unwrap();
        let _cleanup = EnvVarGuard::set("HERMES_HOME", fixture.root());
        let shared = SharedArgs {
            offline: true,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        assert_focused_matches_unified(&crate::SOURCE, &shared, &[]);
    }
}
