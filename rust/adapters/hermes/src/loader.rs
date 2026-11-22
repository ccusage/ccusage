use std::path::PathBuf;

use ccusage_adapter_common::{
    dedupe::dedupe_first_wins,
    source::LoadContext,
    sqlite::sqlite_units,
    units::{SqliteStage, UnitError},
};
use ccusage_core::{Diagnostics, LoadedEntry, Result};

use super::{
    parser::{HermesEntry, read_session_row, to_loaded_entry},
    paths::hermes_state_db_paths,
};

pub(crate) fn load_entries_with_context(cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
    let pricing = cx.pricing.table(cx.shared.mode);
    let tz = cx.window.timezone();
    let db_paths = hermes_state_db_paths()?;
    let loaded = load_state_db_entries(&db_paths, cx.shared.single_thread, cx.diagnostics);
    let mut entries: Vec<_> = dedupe_first_wins(loaded, |entry| entry.session_id.clone())
        .into_iter()
        .map(|entry| to_loaded_entry(entry, tz, cx.shared.mode, pricing))
        .collect();
    LoadedEntry::sort_by_timestamp(&mut entries);
    Ok(entries)
}

fn load_state_db_entries(
    db_paths: &[PathBuf],
    single_thread: bool,
    diagnostics: &Diagnostics,
) -> Vec<HermesEntry> {
    sqlite_units(
        db_paths,
        single_thread,
        diagnostics,
        |connection, _| {
            connection
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
                .map_err(|error| UnitError::sqlite(SqliteStage::Setup, error.code))
        },
        |statement, _, _| {
            read_session_row(statement)
                .map_err(|rejection| UnitError::new(rejection.kind(), rejection.detail()))
        },
    )
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
    use ccusage_test_support::{Environment, fs_fixture};

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
        ccusage_test_support::install_pricing_snapshots();
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
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };
        let tz = ccusage_core::resolve_tz(shared.timezone.as_ref());
        let diagnostics = Diagnostics::new("hermes");
        let entries = load_state_db_entries(std::slice::from_ref(&db_path), true, &diagnostics)
            .into_iter()
            .map(|entry| {
                to_loaded_entry(
                    entry,
                    &tz,
                    shared.mode,
                    ccusage_core::cost::PriceTable::for_map(shared.mode, &pricing),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date.to_string(), "2025-06-15");
        assert_eq!(entries[0].session_id.as_ref(), "session-1");
        assert_eq!(
            entries[0].model.as_deref(),
            Some("claude-sonnet-4-20250514")
        );
        assert_eq!(entries[0].usage.input_tokens, 1200);
        assert_eq!(entries[0].usage.output_tokens, 300);
        assert_eq!(entries[0].usage.cache_creation_input_tokens, 20);
        assert_eq!(entries[0].usage.cache_read_input_tokens, 50);
        assert_eq!(entries[0].extra_total_tokens, 0);
        assert_eq!(entries[0].message_count, Some(42));
        assert_eq!(entries[0].cost, 0.34);
    }

    #[test]
    fn classifies_state_database_query_failures_and_keeps_healthy_sessions() {
        ccusage_test_support::install_pricing_snapshots();
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
        Environment::with_var("HERMES_HOME", homes).run(|| {
            let shared = SharedArgs::default();
            let pricing = PricingMap::load_embedded();
            let window = ReportWindow::from_shared(&shared);
            let diagnostics = Diagnostics::new("hermes");
            let cx = LoadContext {
                shared: &shared,
                kind: AgentReportKind::Daily,
                window: &window,
                pricing: ccusage_adapter_common::source::ReportPricing::Supplied(&pricing),
                options: SourceOptions::default(),
                diagnostics: &diagnostics,
            };

            let entries = crate::SOURCE.load_entries(&cx).unwrap();

            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].session_id.as_ref(), "session-a");
            assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
            assert_eq!(diagnostics.skipped(SkipKind::Invariant), 1);
        })
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

        let entries = load_state_db_entries(std::slice::from_ref(&db_path), true, &diagnostics);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].session_id, "before");
        assert_eq!(entries[1].session_id, "after");
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        assert_eq!(diagnostics.skipped(SkipKind::InvalidTimestamp), 1);
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
    fn counts_a_session_whose_id_is_only_whitespace() {
        let fixture = fs_fixture!({"state.db": ""});
        let db_path = fixture.path("state.db");
        create_state_db(&db_path);
        sqlite::open(&db_path)
            .unwrap()
            .execute(
                "INSERT INTO sessions (id, source, model, started_at, input_tokens, output_tokens) VALUES (' ', 'cli', 'gpt-5', 1750000000, 100, 50)",
            )
            .unwrap();
        let diagnostics = Diagnostics::new("hermes");

        let entries = load_state_db_entries(std::slice::from_ref(&db_path), true, &diagnostics);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id, " ");
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 0);
    }

    #[test]
    fn focused_and_unified_reports_agree() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({"state.db": ""});
        let db_path = fixture.path("state.db");
        create_state_db(&db_path);
        sqlite::open(&db_path)
            .unwrap()
            .execute(
                "INSERT INTO sessions (id, source, model, started_at, input_tokens, output_tokens) VALUES ('session-a', 'cli', 'gpt-5', 1750000000, 100, 50)",
            )
            .unwrap();
        Environment::with_var("HERMES_HOME", fixture.root()).run(|| {
            let shared = SharedArgs {
                offline: true,
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                ..SharedArgs::default()
            };

            assert_focused_matches_unified(
                &crate::SOURCE,
                ccusage_core::cli::AgentId::Hermes,
                &shared,
            );
        })
    }
}
