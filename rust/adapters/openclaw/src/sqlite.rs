use ccusage_core::cost::PriceTable;
use std::path::Path;

use ccusage_adapter_common::{SqliteStage, UnitError, dir_entries, jsonl, sqlite::read_i64};
use ccusage_core::{Diagnostics, Skip, SkipKind};
use jiff::tz::TimeZone as JiffTimeZone;

use super::parser::{OpenClawLine, TranscriptReader, entry_to_loaded, timestamp_from_millis};
use crate::{LoadedEntry, cli::CostMode};

struct AgentDatabase {
    path: std::path::PathBuf,
}

impl AsRef<Path> for AgentDatabase {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

// NOTE(2026-09-16, #1727): OpenClaw 2026.9.x moved live sessions into per-agent SQLite stores. Discovery includes these stores because their live events may have no JSONL copy.
fn collect_agent_databases(root: &Path, diagnostics: &Diagnostics) -> Vec<AgentDatabase> {
    let mut databases = Vec::new();
    let agents_path = root.join("agents");
    if !file_type_without_following_symlinks(&agents_path, diagnostics)
        .is_some_and(|kind| kind.is_dir())
    {
        return databases;
    }
    for (agent, file_type) in dir_entries(&agents_path, diagnostics) {
        if !file_type.is_dir() {
            continue;
        }
        let agent_path = agent.join("agent");
        if !file_type_without_following_symlinks(&agent_path, diagnostics)
            .is_some_and(|kind| kind.is_dir())
        {
            continue;
        }
        let path = agent_path.join("openclaw-agent.sqlite");
        if file_type_without_following_symlinks(&path, diagnostics)
            .is_some_and(|kind| kind.is_file())
        {
            databases.push(AgentDatabase { path });
        }
    }
    databases.sort_by(|left, right| left.path.cmp(&right.path));
    databases
}

fn file_type_without_following_symlinks(
    path: &Path,
    diagnostics: &Diagnostics,
) -> Option<std::fs::FileType> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Some(metadata.file_type()),
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                diagnostics.record(Skip::unit(
                    path,
                    SkipKind::Unreadable,
                    "cannot read file type",
                ));
            }
            None
        }
    }
}

pub(super) fn collect_sqlite_entries(
    root: &Path,
    tz: &JiffTimeZone,
    mode: CostMode,
    pricing: PriceTable<'_>,
    diagnostics: &Diagnostics,
) -> Vec<LoadedEntry> {
    let mut entries = Vec::new();
    for database in collect_agent_databases(root, diagnostics) {
        entries.extend(read_agent_database(
            &database,
            tz,
            mode,
            pricing,
            diagnostics,
        ));
    }
    entries
}

fn read_agent_database(
    database: &AgentDatabase,
    tz: &JiffTimeZone,
    mode: CostMode,
    pricing: PriceTable<'_>,
    diagnostics: &Diagnostics,
) -> Vec<LoadedEntry> {
    let prefilter = super::parser::record_prefilter();
    ccusage_adapter_common::sqlite::sqlite_units_with_state(
        std::slice::from_ref(database),
        true,
        diagnostics,
        |connection, _| {
            match ccusage_adapter_common::sqlite::table_exists(connection, "transcript_events") {
                Ok(true) => {}
                Ok(false) => {
                    return Err(UnitError::new(
                        SkipKind::Invariant,
                        "OpenClaw database has no transcript_events table",
                    ));
                }
                Err(error) => return Err(UnitError::sqlite(SqliteStage::Setup, error.code)),
            }
            connection.prepare("SELECT session_id, seq, event_json, created_at FROM transcript_events ORDER BY session_id ASC, seq ASC")
                .map(|statement| (statement, None::<TranscriptReader>))
                .map_err(|error| UnitError::sqlite(SqliteStage::Setup, error.code))
        },
        |session, statement, database, row_number| {
            let session_id = statement.read::<String, _>(0).map_err(|_| {
                UnitError::new(
                    SkipKind::InvalidData,
                    "OpenClaw transcript row has no session id",
                )
            })?;
            let reader = match session.take() {
                Some(reader) if reader.session_id() == session_id => session.insert(reader),
                _ => session.insert(TranscriptReader::sqlite(session_id)),
            };
            let fallback_timestamp = read_i64(statement, 3).and_then(timestamp_from_millis);
            let event_json = statement.read::<String, _>(2).map_err(|_| {
                UnitError::new(
                    SkipKind::InvalidData,
                    "OpenClaw transcript row has no event data",
                )
            })?;
            if !prefilter.matches(event_json.as_bytes()) {
                return Ok(None);
            }
            let Some(record) = jsonl::decode_record::<OpenClawLine>(
                event_json.as_bytes(),
                &database.path,
                row_number,
                None,
                diagnostics,
                Some(super::parser::is_usage_envelope),
            ) else {
                return Ok(None);
            };
            Ok(reader
                .read(
                    &record,
                    &database.path,
                    row_number,
                    fallback_timestamp,
                    diagnostics,
                )
                .map(|entry| entry_to_loaded(entry, tz, mode, pricing)))
        },
    )
}

#[cfg(test)]
mod tests {
    use ccusage_adapter_common::source::{EntrySource, LoadContext, SourceOptions};
    use ccusage_core::{ReportWindow, cli::AgentReportKind};
    use ccusage_test_support::{Fixture, fs_fixture, read_fixture_dir};

    use super::*;

    fn diagnostics() -> Diagnostics {
        Diagnostics::new("openclaw")
    }

    fn create_agent_db(path: &Path, rows: &[(&str, i64, &str)]) {
        let db = sqlite::open(path).unwrap();
        db.execute(
            "CREATE TABLE transcript_events (session_id TEXT NOT NULL, seq INTEGER NOT NULL, event_json TEXT NOT NULL, created_at INTEGER NOT NULL, PRIMARY KEY (session_id, seq))",
        )
        .unwrap();
        for (session_id, created_at, event_json) in rows {
            let mut statement = db
                .prepare(
                    "INSERT INTO transcript_events (session_id, seq, event_json, created_at) VALUES (?1, (SELECT COALESCE(MAX(seq), -1) + 1 FROM transcript_events WHERE session_id = ?1), ?2, ?3)",
                )
                .unwrap();
            statement.bind((1, *session_id)).unwrap();
            statement.bind((2, *event_json)).unwrap();
            statement.bind((3, *created_at)).unwrap();
            statement.next().unwrap();
        }
    }

    fn sqlite_root(fixture: &Fixture) -> std::path::PathBuf {
        fixture.path("agents/main/agent/openclaw-agent.sqlite")
    }

    #[test]
    fn reads_assistant_usage_from_agent_database() {
        let fixture = fs_fixture!({});
        let db_path = sqlite_root(&fixture);
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        create_agent_db(
            &db_path,
            &[
                (
                    "session-a",
                    1_769_753_935_279,
                    r#"{"id":"evt-1","type":"model_change","modelId":"gpt-5.2","provider":"openai-codex"}"#,
                ),
                (
                    "session-a",
                    1_769_753_935_279,
                    r#"{"id":"evt-2","type":"message","message":{"role":"assistant","usage":{"input":1660,"output":55,"cacheRead":108928,"cost":{"total":0.02}},"timestamp":1769753935279}}"#,
                ),
            ],
        );

        let entries = read_agent_database(
            &AgentDatabase { path: db_path },
            &jiff::tz::TimeZone::UTC,
            CostMode::Auto,
            PriceTable::for_map(CostMode::Auto, &ccusage_core::PricingMap::default()),
            &diagnostics(),
        );

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id.as_ref(), "session-a");
        assert_eq!(entries[0].model.as_deref(), Some("[openclaw] gpt-5.2"));
        assert_eq!(entries[0].version.as_deref(), Some("openai-codex"));
        assert_eq!(entries[0].usage.input_tokens, 1660);
        assert_eq!(entries[0].usage.output_tokens, 55);
        assert_eq!(entries[0].usage.cache_read_input_tokens, 108_928);
        assert!((entries[0].cost - 0.02).abs() < f64::EPSILON);
        assert_eq!(entries[0].message_id.as_deref(), Some("evt-2"));
    }

    #[test]
    fn an_undatable_event_is_skipped_instead_of_dated_by_a_clamped_created_at() {
        let fixture = fs_fixture!({});
        let db_path = sqlite_root(&fixture);
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let usage = r#"{"type":"message","message":{"role":"assistant","model":"gpt-5.2","usage":{"input":10,"output":5}}}"#;
        create_agent_db(&db_path, &[("session-a", 1_769_753_935_279, usage)]);
        let db = sqlite::open(&db_path).unwrap();
        let mut statement = db
            .prepare(
                "INSERT INTO transcript_events (session_id, seq, event_json, created_at) VALUES ('session-a', 1, ?1, 1e300)",
            )
            .unwrap();
        statement.bind((1, usage)).unwrap();
        statement.next().unwrap();
        let diagnostics = diagnostics();

        let entries = read_agent_database(
            &AgentDatabase { path: db_path },
            &jiff::tz::TimeZone::UTC,
            CostMode::Display,
            PriceTable::Display,
            &diagnostics,
        );

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.timestamp.as_millis())
                .collect::<Vec<_>>(),
            [1_769_753_935_279]
        );
        assert_eq!(diagnostics.skipped(SkipKind::InvalidTimestamp), 1);
    }

    #[test]
    fn skips_malformed_rows_without_dropping_the_session() {
        let fixture = fs_fixture!({});
        let db_path = sqlite_root(&fixture);
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        create_agent_db(
            &db_path,
            &[
                (
                    "session-a",
                    1_769_753_935_279,
                    r#"{"id":"evt-1","type":"message","message":{"role":"assistant","model":"gpt-5.2","usage":{"input":10,"output":20},"timestamp":1769753935279}}"#,
                ),
                (
                    "session-a",
                    1_769_753_935_279,
                    "{\"usage\":\"SECRET_CANARY\",}",
                ),
                (
                    "session-a",
                    1_769_753_935_279,
                    r#"{"id":"evt-3","type":"message","message":{"role":"assistant","model":"gpt-5.2","usage":{"input":30,"output":40},"timestamp":1769753935279}}"#,
                ),
            ],
        );

        let diagnostics = diagnostics();
        let entries = read_agent_database(
            &AgentDatabase { path: db_path },
            &jiff::tz::TimeZone::UTC,
            CostMode::Auto,
            PriceTable::for_map(CostMode::Auto, &ccusage_core::PricingMap::default()),
            &diagnostics,
        );

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].usage.input_tokens, 10);
        assert_eq!(entries[1].usage.input_tokens, 30);
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        let warnings = diagnostics
            .warnings(&ccusage_core::cli::SharedArgs {
                debug: true,
                ..Default::default()
            })
            .join("\n");
        assert!(warnings.contains("invalid data"));
        assert!(!warnings.contains("SECRET_CANARY"));
    }

    #[test]
    fn diagnoses_nested_usage_errors_and_skips_events_without_markers() {
        let fixture = fs_fixture!({});
        let db_path = sqlite_root(&fixture);
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        create_agent_db(
            &db_path,
            &[
                (
                    "session-a",
                    1_769_753_935_279,
                    r#"{"type":"message","message":{"role":"assistant","usage":"SECRET_CANARY"}}"#,
                ),
                (
                    "session-a",
                    1_769_753_935_279,
                    r#"{"type":"message","message":{"role":"toolResult","content":"\u001b[31mred"#,
                ),
                (
                    "session-a",
                    1_769_753_935_279,
                    r#"{"type":"message","message":{"role":"assistant","model":"gpt-5.2","usage":{"input":10,"output":20}}}"#,
                ),
            ],
        );
        let diagnostics = diagnostics();
        let entries = read_agent_database(
            &AgentDatabase { path: db_path },
            &jiff::tz::TimeZone::UTC,
            CostMode::Auto,
            PriceTable::for_map(CostMode::Auto, &ccusage_core::PricingMap::default()),
            &diagnostics,
        );

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].usage.input_tokens, 10);
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
    }

    #[test]
    fn malformed_message_object_keeps_sqlite_model_tracking() {
        let fixture = fs_fixture!({});
        let db_path = sqlite_root(&fixture);
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        create_agent_db(
            &db_path,
            &[
                (
                    "session-a",
                    1_769_753_935_279,
                    r#"{"type":"model_change","modelId":"gpt-5.2","provider":"openai","message":{"role":1}}"#,
                ),
                (
                    "session-a",
                    1_769_753_935_279,
                    r#"{"type":"message","message":{"role":"assistant","usage":{"input":10,"output":20}}}"#,
                ),
            ],
        );
        let diagnostics = diagnostics();
        let entries = read_agent_database(
            &AgentDatabase { path: db_path },
            &jiff::tz::TimeZone::UTC,
            CostMode::Auto,
            PriceTable::for_map(CostMode::Auto, &ccusage_core::PricingMap::default()),
            &diagnostics,
        );

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("[openclaw] gpt-5.2"));
        assert_eq!(diagnostics.skipped(SkipKind::UndecodedUsage), 1);
    }

    #[test]
    fn tracks_the_model_per_session() {
        let fixture = fs_fixture!({});
        let db_path = sqlite_root(&fixture);
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        create_agent_db(
            &db_path,
            &[
                (
                    "session-a",
                    1_769_753_935_279,
                    r#"{"type":"model_change","modelId":"gpt-5.2","provider":"openai"}"#,
                ),
                (
                    "session-a",
                    1_769_753_935_279,
                    r#"{"type":"message","message":{"role":"assistant","usage":{"input":10,"output":20}}}"#,
                ),
                (
                    "session-b",
                    1_769_753_935_279,
                    r#"{"type":"message","message":{"role":"assistant","usage":{"input":30,"output":40}}}"#,
                ),
            ],
        );

        let entries = read_agent_database(
            &AgentDatabase { path: db_path },
            &jiff::tz::TimeZone::UTC,
            CostMode::Auto,
            PriceTable::for_map(CostMode::Auto, &ccusage_core::PricingMap::default()),
            &diagnostics(),
        );

        let models = entries
            .iter()
            .map(|entry| (entry.session_id.as_ref(), entry.model.as_deref()))
            .collect::<Vec<_>>();
        assert_eq!(
            models,
            [
                ("session-a", Some("[openclaw] gpt-5.2")),
                ("session-b", Some("[openclaw] unknown")),
            ]
        );
    }

    #[test]
    fn logs_and_ignores_databases_without_transcript_events() {
        let fixture = fs_fixture!({});
        let db_path = sqlite_root(&fixture);
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        sqlite::open(&db_path)
            .unwrap()
            .execute("CREATE TABLE other (id TEXT)")
            .unwrap();
        let diagnostics = diagnostics();

        let entries = read_agent_database(
            &AgentDatabase {
                path: db_path.clone(),
            },
            &jiff::tz::TimeZone::UTC,
            CostMode::Auto,
            PriceTable::for_map(CostMode::Auto, &ccusage_core::PricingMap::default()),
            &diagnostics,
        );

        assert!(entries.is_empty());
        assert_eq!(diagnostics.skipped(SkipKind::Invariant), 1);
    }

    #[test]
    fn reports_sqlite_corruption_during_schema_probe_as_invalid_data() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "agents/main/agent/openclaw-agent.sqlite": "not a SQLite database",
        });
        let db_path = sqlite_root(&fixture);
        let connection = sqlite::Connection::open_with_flags(
            &db_path,
            sqlite::OpenFlags::new().with_read_only(),
        )
        .expect("SQLite should open the damaged file before probing its schema");
        let probe = ccusage_adapter_common::sqlite::table_exists(&connection, "transcript_events");
        assert!(
            probe.is_err(),
            "the schema probe must fail for this fixture"
        );
        drop(connection);

        let custom_path = fixture.root().to_str().unwrap();
        let shared = ccusage_core::cli::SharedArgs {
            single_thread: true,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..Default::default()
        };
        let window = ReportWindow::from_shared(&shared);
        let pricing = ccusage_core::PricingMap::load_embedded();
        let diagnostics = diagnostics();
        let cx = LoadContext {
            shared: &shared,
            kind: AgentReportKind::Daily,
            window: &window,
            pricing: ccusage_adapter_common::source::ReportPricing::Supplied(&pricing),
            options: SourceOptions {
                open_claw_path: Some(custom_path),
                ..SourceOptions::default()
            },
            diagnostics: &diagnostics,
        };

        let entries = crate::SOURCE.load_entries(&cx).unwrap();

        assert!(entries.is_empty());
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 0);
        assert_eq!(diagnostics.skipped(SkipKind::Invariant), 0);
    }

    #[test]
    fn discovers_agent_databases_in_session_order() {
        let fixture = fs_fixture!({
            "agents/main/agent/openclaw-agent.sqlite": "",
            "agents/worker/agent/openclaw-agent.sqlite": "",
            "agents/empty/agent/not-a-database.sqlite": "",
        });

        let root = fixture.root();
        let databases = collect_agent_databases(root, &diagnostics());

        assert_eq!(databases.len(), 2);
        assert_eq!(
            databases[0].path,
            fixture.path("agents/main/agent/openclaw-agent.sqlite")
        );
        assert_eq!(
            databases[1].path,
            fixture.path("agents/worker/agent/openclaw-agent.sqlite")
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_agents_directory_is_reported() {
        use std::{fs, os::unix::fs::PermissionsExt};

        let fixture = fs_fixture!({
            "agents/main/agent/openclaw-agent.sqlite": "",
        });
        let agents = fixture.path("agents");
        fs::set_permissions(&agents, fs::Permissions::from_mode(0o000)).unwrap();
        let denied = read_fixture_dir(&agents).is_err();
        let diagnostics = diagnostics();
        let databases = collect_agent_databases(fixture.root(), &diagnostics);
        fs::set_permissions(&agents, fs::Permissions::from_mode(0o700)).unwrap();
        if !denied {
            return;
        }

        assert!(databases.is_empty());
        assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 1);
    }

    #[cfg(unix)]
    #[test]
    fn ignores_symlinked_database_paths() {
        use std::os::unix::fs::symlink;

        let fixture = fs_fixture!({
            "outside/openclaw-agent.sqlite": "",
            "agents/file-link/agent": "",
            "agents/directory-link": "",
        });
        std::fs::remove_file(fixture.path("agents/file-link/agent")).unwrap();
        std::fs::create_dir_all(fixture.path("agents/file-link/agent")).unwrap();
        symlink(
            fixture.path("outside/openclaw-agent.sqlite"),
            fixture.path("agents/file-link/agent/openclaw-agent.sqlite"),
        )
        .unwrap();
        std::fs::remove_file(fixture.path("agents/directory-link")).unwrap();
        std::fs::create_dir_all(fixture.path("agents/directory-link")).unwrap();
        symlink(
            fixture.path("outside"),
            fixture.path("agents/directory-link/agent"),
        )
        .unwrap();

        assert!(collect_agent_databases(fixture.root(), &diagnostics()).is_empty());
    }
}
