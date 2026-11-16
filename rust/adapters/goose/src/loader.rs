use std::{collections::HashSet, path::Path};

use jiff::tz::TimeZone as JiffTimeZone;

use ccusage_adapter_common::{
    SqliteStage,
    source::LoadContext,
    units::{UnitError, read_units_flat},
};
use ccusage_core::{Diagnostics, LoadedEntry, PricingMap, Result, Skip};

use super::{parser::row_to_entry, paths::goose_db_paths};

const GOOSE_SESSION_QUERY: &str = r#"
SELECT
    id,
    model_config_json,
    provider_name,
    created_at,
    total_tokens,
    input_tokens,
    output_tokens,
    accumulated_total_tokens,
    accumulated_input_tokens,
    accumulated_output_tokens
FROM sessions
WHERE model_config_json IS NOT NULL
    AND TRIM(model_config_json) != ''
"#;

pub(crate) fn load_entries_with_context(cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
    let pricing = cx.pipeline_pricing()?;
    let tz = cx.window.timezone();
    let db_paths = goose_db_paths()?;
    let mut entries = read_units_flat(
        &db_paths,
        cx.shared.single_thread,
        cx.diagnostics,
        |db_path| load_entries_from_db(db_path, tz, pricing, cx.diagnostics),
    );
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

fn load_entries_from_db(
    db_path: &Path,
    tz: &JiffTimeZone,
    pricing: &PricingMap,
    diagnostics: &Diagnostics,
) -> std::result::Result<Vec<LoadedEntry>, UnitError> {
    let connection =
        sqlite::Connection::open_with_flags(db_path, sqlite::OpenFlags::new().with_read_only())
            .map_err(|error| UnitError::sqlite(SqliteStage::Setup, error.code))?;
    let mut statement = connection
        .prepare(GOOSE_SESSION_QUERY)
        .map_err(|error| UnitError::sqlite(SqliteStage::Setup, error.code))?;

    let mut entries = Vec::new();
    let mut seen_sessions = HashSet::new();
    let mut row_number = 0_u32;
    loop {
        match statement.next() {
            Ok(sqlite::State::Row) => {
                row_number = row_number.saturating_add(1);
                match row_to_entry(&statement, tz, pricing) {
                    Ok(Some(entry)) => {
                        if seen_sessions.insert(entry.session_id.clone()) {
                            entries.push(entry);
                        }
                    }
                    Ok(None) => {}
                    Err(rejection) => diagnostics.record(Skip::record(
                        db_path,
                        row_number,
                        rejection.kind(),
                        rejection.detail(),
                    )),
                }
            }
            Ok(sqlite::State::Done) => break,
            Err(error) => {
                UnitError::sqlite(SqliteStage::Rows, error.code).record(db_path, diagnostics);
                break;
            }
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use crate::cli::SharedArgs;
    use std::path::Path;

    use super::*;
    use ccusage_adapter_common::{parity::assert_focused_matches_unified, source::SourceOptions};
    use ccusage_core::{ReportWindow, SkipKind, cli::AgentReportKind};
    use ccusage_test_support::{EnvVarGuard, EnvVarsGuard, fs_fixture};

    fn create_goose_db(path: &Path) {
        let db = sqlite::open(path).unwrap();
        db.execute(
            r#"
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    model_config_json TEXT,
    provider_name TEXT,
    created_at TEXT,
    total_tokens INTEGER,
    input_tokens INTEGER,
    output_tokens INTEGER,
    accumulated_total_tokens INTEGER,
    accumulated_input_tokens INTEGER,
    accumulated_output_tokens INTEGER
)
"#,
        )
        .unwrap();
    }

    struct SessionFixture<'a> {
        id: &'a str,
        model_config: &'a str,
        provider: Option<&'a str>,
        created_at: &'a str,
        total: i64,
        input: i64,
        output: i64,
    }

    fn insert_session(path: &Path, fixture: SessionFixture<'_>) {
        let db = sqlite::open(path).unwrap();
        let mut statement = db
            .prepare(
                r#"
INSERT INTO sessions (
    id,
    model_config_json,
    provider_name,
    created_at,
    accumulated_total_tokens,
    accumulated_input_tokens,
    accumulated_output_tokens
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
"#,
            )
            .unwrap();
        statement.bind((1, fixture.id)).unwrap();
        statement.bind((2, fixture.model_config)).unwrap();
        statement.bind((3, fixture.provider)).unwrap();
        statement.bind((4, fixture.created_at)).unwrap();
        statement.bind((5, fixture.total)).unwrap();
        statement.bind((6, fixture.input)).unwrap();
        statement.bind((7, fixture.output)).unwrap();
        statement.next().unwrap();
    }

    #[test]
    fn loads_accumulated_tokens_from_goose_sqlite() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path(super::super::paths::GOOSE_DB_FILE_NAME);
        create_goose_db(&db_path);
        insert_session(
            &db_path,
            SessionFixture {
                id: "session-a",
                model_config: r#"{"model_name":"claude-sonnet-4-20250514"}"#,
                provider: Some("anthropic"),
                created_at: "2026-05-01 01:02:03",
                total: 180,
                input: 100,
                output: 50,
            },
        );

        let pricing = PricingMap::load_embedded();
        let diagnostics = Diagnostics::new("goose");
        let entries =
            load_entries_from_db(&db_path, &jiff::tz::TimeZone::UTC, &pricing, &diagnostics)
                .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2026-05-01");
        assert_eq!(entries[0].session_id.as_ref(), "session-a");
        assert_eq!(entries[0].data.message.usage.input_tokens, 100);
        assert_eq!(entries[0].data.message.usage.output_tokens, 50);
        assert_eq!(entries[0].extra_total_tokens, 30);
    }

    #[test]
    fn diagnoses_sessions_with_usage_it_cannot_place_or_attribute() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path(super::super::paths::GOOSE_DB_FILE_NAME);
        create_goose_db(&db_path);
        let session = |id, model_config, created_at, total| SessionFixture {
            id,
            model_config,
            provider: Some("openai"),
            created_at,
            total,
            input: total,
            output: 0,
        };
        let gpt = r#"{"model_name":"gpt-5"}"#;
        insert_session(&db_path, session("healthy", gpt, "2026-05-01 01:02:03", 10));
        insert_session(&db_path, session("bad-time", gpt, "SECRET_CANARY", 10));
        insert_session(
            &db_path,
            session("no-model", "{}", "2026-05-01 01:02:03", 10),
        );
        insert_session(&db_path, session("idle", "{}", "SECRET_CANARY", 0));

        let diagnostics = Diagnostics::new("goose");
        let entries = load_entries_from_db(
            &db_path,
            &jiff::tz::TimeZone::UTC,
            &PricingMap::default(),
            &diagnostics,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id.as_ref(), "healthy");
        assert_eq!(diagnostics.skipped(SkipKind::InvalidTimestamp), 1);
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        let warnings = diagnostics
            .warnings(&SharedArgs {
                debug: true,
                ..SharedArgs::default()
            })
            .join("\n");
        assert!(!warnings.contains("SECRET_CANARY"));
    }

    #[test]
    fn a_database_without_the_session_columns_is_an_unsupported_schema() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("sessions.db");
        sqlite::open(&db_path)
            .unwrap()
            .execute("CREATE TABLE sessions (id TEXT)")
            .unwrap();

        let error = load_entries_from_db(
            &db_path,
            &jiff::tz::TimeZone::UTC,
            &PricingMap::default(),
            &Diagnostics::new("goose"),
        )
        .unwrap_err();
        let diagnostics = Diagnostics::new("goose");
        error.record(&db_path, &diagnostics);

        assert_eq!(diagnostics.skipped(SkipKind::Invariant), 1);
    }

    #[test]
    fn a_failed_session_database_does_not_hide_healthy_sessions() {
        let fixture = fs_fixture!({
            ".local/share/goose/sessions/sessions.db": "",
            ".local/share/Block/goose/sessions/sessions.db": "not a SQLite database",
        });
        let good_db = fixture.path(".local/share/goose/sessions/sessions.db");
        create_goose_db(&good_db);
        insert_session(
            &good_db,
            SessionFixture {
                id: "session-a",
                model_config: r#"{"model_name":"gpt-5"}"#,
                provider: Some("openai"),
                created_at: "2026-05-01 01:02:03",
                total: 150,
                input: 100,
                output: 50,
            },
        );
        let _environment = EnvVarsGuard::set_many([
            ("HOME", Some(fixture.root().to_path_buf().into_os_string())),
            ("GOOSE_PATH_ROOT", Some(std::ffi::OsString::new())),
        ]);
        let shared = SharedArgs::default();
        let pricing = PricingMap::load_embedded();
        let window = ReportWindow::from_shared(&shared);
        let diagnostics = Diagnostics::new("goose");
        let cx = LoadContext {
            shared: &shared,
            kind: AgentReportKind::Daily,
            window: &window,
            pricing: Some(&pricing),
            options: SourceOptions::default(),
            diagnostics: &diagnostics,
        };

        let entries = load_entries_with_context(&cx).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id.as_ref(), "session-a");
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
    }

    #[test]
    fn focused_and_unified_reports_agree() {
        let fixture = fs_fixture!({"data/sessions/sessions.db": ""});
        let db_path = fixture.path("data/sessions/sessions.db");
        create_goose_db(&db_path);
        insert_session(
            &db_path,
            SessionFixture {
                id: "session-a",
                model_config: r#"{"model_name":"gpt-5"}"#,
                provider: Some("openai"),
                created_at: "2026-05-01 01:02:03",
                total: 150,
                input: 100,
                output: 50,
            },
        );
        let _cleanup = EnvVarGuard::set("GOOSE_PATH_ROOT", fixture.root());
        let shared = SharedArgs {
            offline: true,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        assert_focused_matches_unified(&crate::SOURCE, &shared, &[]);
    }
}
