use std::path::Path;

use jiff::tz::TimeZone as JiffTimeZone;

use super::parser::{OpenClawLine, parse_transcript_message, sqlite_entry_to_loaded};
use crate::{LoadedEntry, PricingMap, TimestampMs, cli::CostMode};

/// One per-agent SQLite store (`agents/<agentId>/agent/openclaw-agent.sqlite`).
struct AgentDatabase {
    path: std::path::PathBuf,
}

/// Find every per-agent SQLite store under `root`.
///
/// OpenClaw resolves the agent database as
/// `<stateDir>/agents/<agentId>/agent/openclaw-agent.sqlite`, and
/// `~/.openclaw` is the default state directory, so each configured root is
/// scanned for `agents/*/agent/openclaw-agent.sqlite`. Directory traversal
/// stays lexical (no symlink following, no canonicalization): discovery must
/// agree with the JSONL walk in `paths.rs`, which also refuses symlinks.
/// Reads open read-only.
fn collect_agent_databases(root: &Path) -> Vec<AgentDatabase> {
    let mut databases = Vec::new();
    let agents_path = root.join("agents");
    if !is_directory_without_symlink(&agents_path) {
        return databases;
    }
    let Ok(agents) = std::fs::read_dir(agents_path) else {
        return databases;
    };
    for agent in agents.filter_map(std::result::Result::ok) {
        let Ok(file_type) = agent.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let agent_path = agent.path().join("agent");
        if !is_directory_without_symlink(&agent_path) {
            continue;
        }
        let path = agent_path.join("openclaw-agent.sqlite");
        if is_file_without_symlink(&path) {
            databases.push(AgentDatabase { path });
        }
    }
    databases.sort_by(|left, right| left.path.cmp(&right.path));
    databases
}

fn is_directory_without_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_dir())
}

fn is_file_without_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

pub(super) fn collect_sqlite_entries(
    root: &Path,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
    debug: &dyn Fn(String),
) -> Vec<LoadedEntry> {
    let mut entries = Vec::new();
    for database in collect_agent_databases(root) {
        entries.extend(read_agent_database(&database, tz, mode, pricing, debug));
    }
    entries
}

/// Read one agent database, returning usage entries in `(session_id, seq)`
/// order.
///
/// The connection opens read-only, so inspecting a hostile store cannot write
/// to it. Rows whose `event_json` fails to parse are skipped: transcript
/// events are an append-only log shared with redaction and branch-control
/// records, and a single malformed row must not drop the session around it.
fn read_agent_database(
    database: &AgentDatabase,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
    debug: &dyn Fn(String),
) -> Vec<LoadedEntry> {
    let Ok(connection) = sqlite::Connection::open_with_flags(
        &database.path,
        sqlite::OpenFlags::new().with_read_only(),
    ) else {
        debug(format!(
            "Failed to open OpenClaw agent database: {}",
            database.path.display()
        ));
        return Vec::new();
    };
    if !table_exists(&connection) {
        debug(format!(
            "OpenClaw agent database has no transcript_events table: {}",
            database.path.display()
        ));
        return Vec::new();
    }
    let Ok(mut statement) = connection.prepare(
        "SELECT session_id, seq, event_json, created_at FROM transcript_events ORDER BY session_id ASC, seq ASC",
    ) else {
        debug(format!(
            "Failed to query OpenClaw agent database: {}",
            database.path.display()
        ));
        return Vec::new();
    };
    // Model/provider tracking is per session in the JSONL path (state resets
    // per file). SQLite rows arrive in session order, so track the current
    // session and reset the tracked model/provider on session boundaries.
    let mut current_session: Option<String> = None;
    let mut current_model: Option<String> = None;
    let mut current_provider: Option<String> = None;
    let mut entries = Vec::new();
    loop {
        match statement.next() {
            Ok(sqlite::State::Row) => {
                let Ok(session_id) = statement.read::<String, _>(0) else {
                    continue;
                };
                if current_session.as_deref() != Some(&session_id) {
                    current_session = Some(session_id.clone());
                    current_model = None;
                    current_provider = None;
                }
                let created_at = statement
                    .read::<i64, _>(3)
                    .or_else(|_| {
                        statement
                            .read::<f64, _>(3)
                            .map(|value| value.trunc() as i64)
                    })
                    .unwrap_or(0);
                let fallback_timestamp = TimestampMs::from_millis(created_at.max(0));
                let Ok(event_json) = statement.read::<String, _>(2) else {
                    continue;
                };
                let Ok(record) = serde_json::from_str::<OpenClawLine>(&event_json) else {
                    continue;
                };
                if super::parser::is_model_change(&record) {
                    let (model, provider) = super::parser::model_change_source(&record);
                    if let Some(model) = model {
                        current_model = Some(model);
                    }
                    if let Some(provider) = provider {
                        current_provider = Some(provider);
                    }
                    continue;
                }
                let Some((entry, message_id)) = parse_transcript_message(
                    &record,
                    &session_id,
                    current_model.as_deref(),
                    current_provider.as_deref(),
                    fallback_timestamp,
                ) else {
                    continue;
                };
                entries.push(sqlite_entry_to_loaded(entry, message_id, tz, mode, pricing));
            }
            Ok(sqlite::State::Done) => break,
            Err(_) => {
                debug(format!(
                    "Failed to read OpenClaw agent database: {}",
                    database.path.display()
                ));
                break;
            }
        }
    }
    entries
}

fn table_exists(connection: &sqlite::Connection) -> bool {
    let Ok(mut statement) = connection
        .prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1")
    else {
        return false;
    };
    if statement.bind((1, "transcript_events")).is_err() {
        return false;
    }
    matches!(statement.next(), Ok(sqlite::State::Row))
}

#[cfg(test)]
mod tests {
    use ccusage_test_support::{Fixture, fs_fixture};

    use super::*;

    fn no_debug(_: String) {}

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
            None,
            CostMode::Auto,
            None,
            &no_debug,
        );

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id.as_ref(), "session-a");
        assert_eq!(entries[0].model.as_deref(), Some("[openclaw] gpt-5.2"));
        assert_eq!(entries[0].data.version.as_deref(), Some("openai-codex"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 1660);
        assert_eq!(entries[0].data.message.usage.output_tokens, 55);
        assert_eq!(
            entries[0].data.message.usage.cache_read_input_tokens,
            108_928
        );
        assert!((entries[0].cost - 0.02).abs() < f64::EPSILON);
        assert_eq!(entries[0].data.message.id.as_deref(), Some("evt-2"));
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
                ("session-a", 1_769_753_935_279, "not json"),
                (
                    "session-a",
                    1_769_753_935_279,
                    r#"{"id":"evt-3","type":"message","message":{"role":"assistant","model":"gpt-5.2","usage":{"input":30,"output":40},"timestamp":1769753935279}}"#,
                ),
            ],
        );

        let entries = read_agent_database(
            &AgentDatabase { path: db_path },
            None,
            CostMode::Auto,
            None,
            &no_debug,
        );

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].data.message.usage.input_tokens, 10);
        assert_eq!(entries[1].data.message.usage.input_tokens, 30);
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
        let debug = std::cell::RefCell::new(Vec::new());

        let entries = read_agent_database(
            &AgentDatabase {
                path: db_path.clone(),
            },
            None,
            CostMode::Auto,
            None,
            &|message| debug.borrow_mut().push(message),
        );

        assert!(entries.is_empty());
        assert_eq!(
            debug.into_inner(),
            vec![format!(
                "OpenClaw agent database has no transcript_events table: {}",
                db_path.display()
            )]
        );
    }

    #[test]
    fn discovers_agent_databases_in_session_order() {
        let fixture = fs_fixture!({
            "agents/main/agent/openclaw-agent.sqlite": "",
            "agents/worker/agent/openclaw-agent.sqlite": "",
            "agents/empty/agent/not-a-database.sqlite": "",
        });

        let root = fixture.root();
        let databases = collect_agent_databases(root);

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

        assert!(collect_agent_databases(fixture.root()).is_empty());
    }
}
