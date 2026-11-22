use std::ffi::OsString;
use std::path::{Path, PathBuf};

use ccusage_test_support::{instant, set_modified};

use crate::golden::Case;

const SONNET: &str = "claude-sonnet-4-20250514";
const OPUS: &str = "claude-opus-4-20250514";
const HAIKU: &str = "claude-haiku-4-5-20251001";
const UNPRICED: &str = "opencode-mystery-1";

fn flat(model: &str, provider: &str) -> String {
    format!(r#""modelID":"{model}","providerID":"{provider}""#)
}

fn nested(model: &str, provider: &str) -> String {
    format!(r#""model":{{"id":"{model}","providerID":"{provider}"}}"#)
}

fn tokens(input: u64, output: u64, cache_read: u64, cache_write: u64) -> String {
    format!(
        r#"{{"input":{input},"output":{output},"cache":{{"read":{cache_read},"write":{cache_write}}}}}"#
    )
}

fn payload(model: &str, created: Option<i64>, tokens: &str, extra: &str) -> String {
    let time = created.map_or_else(String::new, |created| {
        format!(r#""time":{{"created":{created}}},"#)
    });
    format!(r#"{{"role":"assistant",{model},{time}"tokens":{tokens}{extra}}}"#)
}

fn claude_agent(native_session: &str, created: i64, input: u64, output: u64) -> String {
    payload(
        &nested("opus", "claude-agent"),
        Some(created),
        &tokens(input, output, 0, 0),
        &format!(
            r#","content":[{{"type":"text","text":"hi","native":{{"sessionID":"{native_session}","messageID":"m"}}}}],"cost":0"#
        ),
    )
}

fn sql_text(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn sql_integer(value: Option<i64>) -> String {
    value.map_or_else(|| "NULL".to_string(), |value| value.to_string())
}

struct Database(sqlite::Connection);

impl Database {
    fn create(path: &Path, schema: &[&str]) -> Self {
        let connection = sqlite::open(path).expect("failed to create OpenCode fixture database");
        for statement in schema {
            connection
                .execute(statement)
                .expect("failed to create OpenCode fixture table");
        }
        Self(connection)
    }

    fn execute(&self, statement: &str) {
        self.0
            .execute(statement)
            .expect("failed to insert OpenCode fixture row");
    }

    fn message(&self, id: &str, session: &str, created: i64, data: &str) {
        self.execute(&format!(
            "INSERT INTO message (id, session_id, time_created, data) VALUES ({}, {}, {created}, {})",
            sql_text(id),
            sql_text(session),
            sql_text(data),
        ));
    }

    fn legacy_message(&self, id: &str, session: &str, data: &str) {
        self.execute(&format!(
            "INSERT INTO message (id, session_id, data) VALUES ({}, {}, {})",
            sql_text(id),
            sql_text(session),
            sql_text(data),
        ));
    }

    fn session_message(
        &self,
        id: &str,
        session: &str,
        row_type: &str,
        seq: i64,
        created: Option<i64>,
        data: &str,
    ) {
        self.execute(&format!(
            "INSERT INTO session_message (id, session_id, type, seq, time_created, data) \
             VALUES ({}, {}, {}, {seq}, {}, {})",
            sql_text(id),
            sql_text(session),
            sql_text(row_type),
            sql_integer(created),
            sql_text(data),
        ));
    }

    fn aggregate(&self, table: &str, id: &str, created: Option<i64>, model: &str, counters: &str) {
        self.execute(&format!(
            "INSERT INTO {table} (id, time_created, model, cost, tokens_input, tokens_output, \
             tokens_cache_read, tokens_cache_write, tokens_reasoning) VALUES ({}, {}, {}, {counters})",
            sql_text(id),
            sql_integer(created),
            sql_text(model),
        ));
    }

    fn fork(&self, id: &str, parent: &str, boundary: &str) {
        self.execute(&format!(
            "UPDATE session_v2 SET fork_session_id = {}, fork_boundary = {} WHERE id = {}",
            sql_text(parent),
            sql_text(boundary),
            sql_text(id),
        ));
    }
}

const AGGREGATE_COLUMNS: &str = "id TEXT PRIMARY KEY, time_created INTEGER, cost REAL, \
    tokens_input INTEGER, tokens_output INTEGER, tokens_cache_read INTEGER, \
    tokens_cache_write INTEGER, tokens_reasoning INTEGER, model TEXT";

fn seed_compaction_as_only_message(db: &Database) {
    db.session_message(
        "msg-v2-compaction-only",
        "ses-v2-compact",
        "compaction",
        0,
        Some(instant("2026-01-01T10:30:00Z").millis()),
        &payload(
            &nested("gpt-5", "openai"),
            Some(instant("2026-01-01T10:30:00Z").millis()),
            &tokens(60, 6, 0, 0),
            r#","status":"completed""#,
        ),
    );
}

fn seed_messages_with_and_without_column_timestamps(db: &Database) {
    let misc = "ses-v2-misc";
    db.session_message(
        "msg-v2-column-time",
        misc,
        "assistant",
        0,
        Some(instant("2026-01-02T11:00:00Z").millis()),
        &payload(
            r#""model":{"modelID":"gpt-5","providerID":"openai"}"#,
            None,
            &tokens(45, 5, 0, 0),
            "",
        ),
    );
    db.session_message(
        "msg-v2-no-time",
        misc,
        "assistant",
        1,
        None,
        &payload(&nested("gpt-5", "openai"), None, &tokens(11, 1, 0, 0), ""),
    );
}

fn seed_claude_agent_messages_with_transcript_overlap(db: &Database) {
    let agent = "ses-v2-agent";
    db.session_message(
        "msg-v2-agent-counted",
        agent,
        "assistant",
        0,
        Some(instant("2025-12-31T12:05:00Z").millis()),
        &claude_agent(
            "claude-native-a",
            instant("2025-12-31T12:05:00Z").millis(),
            5000,
            500,
        ),
    );
    db.session_message(
        "msg-v2-agent-other-day",
        agent,
        "assistant",
        1,
        Some(instant("2026-01-01T12:00:00Z").millis()),
        &claude_agent(
            "claude-native-a",
            instant("2026-01-01T12:00:00Z").millis(),
            64,
            6,
        ),
    );
    db.session_message(
        "msg-v2-agent-no-transcript",
        agent,
        "assistant",
        2,
        Some(instant("2026-01-01T12:10:00Z").millis()),
        &claude_agent(
            "claude-native-missing",
            instant("2026-01-01T12:10:00Z").millis(),
            7,
            1,
        ),
    );
}

fn seed_main_database(path: &Path) {
    let session_v2 = format!(
        "CREATE TABLE session_v2 ({AGGREGATE_COLUMNS}, fork_session_id TEXT, fork_boundary TEXT)"
    );
    let session = format!("CREATE TABLE session ({AGGREGATE_COLUMNS})");
    let db = Database::create(
        path,
        &[
            "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT)",
            "CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT, seq INTEGER NOT NULL DEFAULT 0, time_created INTEGER, data TEXT)",
            &session_v2,
            &session,
            "CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, data TEXT)",
        ],
    );
    db.execute(
        r#"INSERT INTO part VALUES ('part-1', 'msg-v1-a1', '{"type":"step-finish","tokens":{"input":9999,"output":9999}}')"#,
    );

    let alpha = "ses-v1-alpha";
    db.message(
        "msg-v1-a1",
        alpha,
        instant("2025-12-30T10:00:00Z").millis(),
        &payload(
            &flat(SONNET, "anthropic"),
            Some(instant("2025-12-30T10:00:00Z").millis()),
            &tokens(1000, 200, 4000, 300),
            r#","cost":0.5"#,
        ),
    );
    db.message(
        "msg-v1-a2",
        alpha,
        instant("2025-12-31T15:30:00Z").millis(),
        &payload(
            &flat("gpt-5", "openai"),
            Some(instant("2025-12-31T15:30:00Z").millis()),
            r#"{"input":700,"output":150,"reasoning":50,"cache":{"read":2500,"write":0}}"#,
            "",
        ),
    );
    db.message(
        "msg-v1-a3",
        alpha,
        instant("2026-01-01T12:00:00Z").millis(),
        &payload(
            &flat(UNPRICED, "mystery"),
            Some(instant("2026-01-01T12:00:00Z").millis()),
            &tokens(300, 70, 0, 0),
            "",
        ),
    );
    db.message(
        "msg-v1-alias",
        alpha,
        instant("2026-01-01T13:00:00Z").millis(),
        &payload(
            &flat("gemini-3-pro-high", "google"),
            Some(instant("2026-01-01T13:00:00Z").millis()),
            &tokens(200, 20, 0, 0),
            "",
        ),
    );
    db.message(
        "msg-v1-total",
        alpha,
        instant("2026-01-02T10:00:00Z").millis(),
        &payload(
            &flat("claude-haiku-4.5", "anthropic"),
            Some(instant("2026-01-02T10:00:00Z").millis()),
            r#"{"input":100,"output":0,"total":345}"#,
            r#","cost":0"#,
        ),
    );
    let ignored_at = instant("2025-12-31T12:00:00Z").millis();
    db.message(
        "msg-v1-user",
        alpha,
        ignored_at,
        &format!(
            r#"{{"role":"user","time":{{"created":{ignored_at}}},"tokens":{{"input":9999,"output":9999}}}}"#
        ),
    );
    db.message(
        "msg-v1-no-provider",
        alpha,
        ignored_at,
        &format!(
            r#"{{"role":"assistant","modelID":"{SONNET}","time":{{"created":{ignored_at}}},"tokens":{{"input":7777,"output":7777}}}}"#
        ),
    );
    db.message(
        "msg-v1-zero",
        alpha,
        ignored_at,
        &payload(
            &flat(SONNET, "anthropic"),
            Some(ignored_at),
            &tokens(0, 0, 0, 0),
            r#","cost":7.77"#,
        ),
    );
    db.message(
        "msg-v1-odd-tokens",
        alpha,
        ignored_at,
        &payload(
            &flat(SONNET, "anthropic"),
            Some(ignored_at),
            r#""9999""#,
            "",
        ),
    );
    db.message(
        "msg-v1-truncated",
        alpha,
        ignored_at,
        &format!(r#"{{"role":"assistant","modelID":"{SONNET}","tokens":{{"input":9999,"outp"#),
    );

    let parent = "ses-v2-parent";
    let opus = nested(OPUS, "anthropic");
    let turn = |input: u64, output: u64, created: i64, extra: &str| {
        payload(&opus, Some(created), &tokens(input, output, 0, 0), extra)
    };
    db.session_message(
        "msg-v2-p1",
        parent,
        "assistant",
        0,
        Some(instant("2025-12-30T11:00:00Z").millis()),
        &payload(
            &opus,
            Some(instant("2025-12-30T11:00:00Z").millis()),
            &tokens(500, 100, 1000, 200),
            r#","cost":0.25"#,
        ),
    );
    db.session_message(
        "msg-v2-p2",
        parent,
        "assistant",
        1,
        Some(instant("2025-12-31T09:00:00Z").millis()),
        &turn(
            600,
            120,
            instant("2025-12-31T09:00:00Z").millis(),
            r#","cost":0"#,
        ),
    );
    db.session_message(
        "msg-v2-p3",
        parent,
        "assistant",
        2,
        Some(instant("2025-12-31T16:00:00Z").millis()),
        &turn(700, 140, instant("2025-12-31T16:00:00Z").millis(), ""),
    );
    db.session_message(
        "msg-v2-p-compaction",
        parent,
        "compaction",
        3,
        Some(instant("2025-12-31T17:00:00Z").millis()),
        &payload(
            &nested("gpt-5", "openai"),
            Some(instant("2025-12-31T17:00:00Z").millis()),
            &tokens(90, 9, 0, 0),
            r#","status":"completed""#,
        ),
    );
    db.session_message(
        "msg-v2-p4",
        parent,
        "assistant",
        5,
        Some(instant("2026-01-01T08:00:00Z").millis()),
        &payload(
            &flat(SONNET, "anthropic"),
            Some(instant("2026-01-01T08:00:00Z").millis()),
            &tokens(800, 160, 0, 0),
            "",
        ),
    );
    db.session_message(
        "msg-v2-p-compaction-running",
        parent,
        "compaction",
        4,
        Some(ignored_at),
        &payload(
            &nested("gpt-5", "openai"),
            Some(ignored_at),
            &tokens(9999, 9999, 0, 0),
            r#","status":"running""#,
        ),
    );
    db.session_message(
        "msg-v2-p-user",
        parent,
        "user",
        6,
        Some(ignored_at),
        &turn(9999, 9999, ignored_at, ""),
    );
    db.session_message(
        "msg-v2-p-tool",
        parent,
        "tool-result",
        7,
        Some(ignored_at),
        &turn(9999, 9999, ignored_at, ""),
    );
    db.session_message(
        "msg-v2-p-truncated",
        parent,
        "assistant",
        8,
        Some(ignored_at),
        r#"{"model":{"id":"gpt-5","providerID":"openai"},"tokens":{"input":9999,"#,
    );
    db.session_message(
        "msg-v1-a1",
        alpha,
        "assistant",
        0,
        Some(instant("2025-12-30T10:00:00Z").millis()),
        &turn(9999, 9999, instant("2025-12-30T10:00:00Z").millis(), ""),
    );

    let copy = |id: &str, session: &str, seq: i64, input: u64| {
        db.session_message(
            id,
            session,
            "assistant",
            seq,
            Some(instant("2026-01-01T09:00:00Z").millis()),
            &turn(input, 9000, instant("2026-01-01T09:00:00Z").millis(), ""),
        );
    };
    let through = "ses-v2-fork-through";
    copy("msg-v2-ft-copy-1", through, 0, 9001);
    copy("msg-v2-ft-copy-2", through, 1, 9002);
    copy("msg-v2-ft-copy-3", through, 2, 9003);
    db.session_message(
        "msg-v2-ft-own",
        through,
        "assistant",
        6,
        Some(instant("2026-01-01T09:00:00Z").millis()),
        &turn(40, 4, instant("2026-01-01T09:00:00Z").millis(), ""),
    );
    let before = "ses-v2-fork-before";
    copy("msg-v2-fb-copy-1", before, 0, 9004);
    copy("msg-v2-fb-copy-2", before, 1, 9005);
    db.session_message(
        "msg-v2-fb-own",
        before,
        "assistant",
        2,
        Some(instant("2026-01-01T09:30:00Z").millis()),
        &turn(33, 3, instant("2026-01-01T09:30:00Z").millis(), ""),
    );
    let lost = "ses-v2-fork-lost";
    db.session_message(
        "msg-v2-fl-kept",
        lost,
        "assistant",
        0,
        Some(instant("2026-01-01T10:00:00Z").millis()),
        &turn(21, 2, instant("2026-01-01T10:00:00Z").millis(), ""),
    );
    let idle = "ses-v2-fork-idle";
    copy("msg-v2-fi-copy-1", idle, 1, 9006);

    seed_compaction_as_only_message(&db);

    seed_messages_with_and_without_column_timestamps(&db);

    seed_claude_agent_messages_with_transcript_overlap(&db);

    let opus_model = format!(r#"{{"id":"{OPUS}","providerID":"anthropic"}}"#);
    let covered = "0.99, 9999, 9999, 9999, 9999, 0";
    for session in [
        parent,
        through,
        before,
        lost,
        "ses-agg-json",
        "ses-agg-xdir",
    ] {
        db.aggregate(
            "session_v2",
            session,
            Some(instant("2025-12-30T11:00:00Z").millis()),
            &opus_model,
            covered,
        );
    }
    db.aggregate(
        "session_v2",
        idle,
        Some(instant("2026-01-02T09:00:00Z").millis()),
        &opus_model,
        "0.05, 55, 5, 0, 0, 0",
    );
    db.aggregate(
        "session_v2",
        "ses-v2-compact",
        Some(instant("2026-01-01T10:30:00Z").millis()),
        r#"{"modelID":"gpt-5","provider":"openai"}"#,
        "0, 160, 16, 0, 0, 0",
    );
    db.aggregate(
        "session_v2",
        "ses-agg-only",
        Some(instant("2025-12-30T13:00:00Z").millis()),
        &format!(r#"{{"id":"{SONNET}","providerID":"anthropic"}}"#),
        "0.4, 400, 80, 900, 100, 20",
    );
    db.aggregate(
        "session_v2",
        "ses-agg-cost-only",
        Some(instant("2025-12-31T18:00:00Z").millis()),
        "gpt-5",
        "0.33, 0, 0, 0, 0, 0",
    );
    db.aggregate(
        "session_v2",
        "ses-agg-zero",
        Some(instant("2025-12-31T18:00:00Z").millis()),
        "gpt-5",
        "0, 0, 0, 0, 0, 0",
    );
    db.aggregate(
        "session_v2",
        "ses-agg-no-time",
        None,
        "gpt-5",
        "0.88, 8888, 8888, 0, 0, 0",
    );
    db.fork(
        through,
        parent,
        r#"{"type":"through","messageID":"msg-v2-p3"}"#,
    );
    db.fork(
        before,
        parent,
        r#"{"type":"before","messageID":"msg-v2-p3"}"#,
    );
    db.fork(
        lost,
        parent,
        r#"{"type":"through","messageID":"msg-v2-gone"}"#,
    );
    db.fork(
        idle,
        parent,
        r#"{"type":"through","messageID":"msg-v2-p3"}"#,
    );

    db.aggregate(
        "session",
        "ses-legacy-agg",
        Some(instant("2026-01-02T13:00:00Z").millis()),
        UNPRICED,
        "0, 70, 7, 0, 0, 0",
    );
    for session in ["ses-agg-only", alpha] {
        db.aggregate(
            "session",
            session,
            Some(instant("2025-12-30T13:00:00Z").millis()),
            &opus_model,
            covered,
        );
    }
}

fn seed_channel_database(directory: &Path) {
    let schema = ["CREATE TABLE message (id TEXT, session_id TEXT, data TEXT)"];
    let db = Database::create(&directory.join("opencode-beta.db"), &schema);
    let gamma = "ses-ch-gamma";
    db.legacy_message(
        "msg-ch-1",
        gamma,
        &payload(
            &flat(SONNET, "anthropic"),
            Some(instant("2026-01-01T00:10:00Z").millis()),
            &tokens(600, 100, 3000, 0),
            "",
        ),
    );
    db.legacy_message(
        "msg-ch-dup",
        gamma,
        &payload(
            &flat("k2p6", "moonshot"),
            Some(instant("2026-01-02T08:00:00Z").millis()),
            &tokens(250, 25, 0, 0),
            "",
        ),
    );
    db.legacy_message(
        "msg-ch-dup",
        gamma,
        &payload(
            &flat("k2p6", "moonshot"),
            Some(instant("2026-01-02T08:00:00Z").millis()),
            &tokens(9999, 9999, 0, 0),
            "",
        ),
    );
    db.legacy_message(
        "msg-v1-a3",
        "ses-v1-alpha",
        &payload(
            &flat(UNPRICED, "mystery"),
            Some(instant("2026-01-01T12:00:00Z").millis()),
            &tokens(9999, 9999, 0, 0),
            "",
        ),
    );
    db.legacy_message(
        "msg-ch-xdir",
        "ses-agg-xdir",
        &payload(
            &flat(SONNET, "anthropic"),
            Some(instant("2025-12-30T09:00:00Z").millis()),
            &tokens(30, 3, 0, 0),
            "",
        ),
    );

    let later = Database::create(&directory.join("opencode-zeta.db"), &schema);
    later.legacy_message(
        "msg-ch-later",
        gamma,
        &payload(
            &flat(SONNET, "anthropic"),
            Some(instant("2026-01-01T00:20:00Z").millis()),
            &tokens(9999, 9999, 0, 0),
            "",
        ),
    );
}

fn seed_message_files(directory: &Path) {
    let write = |relative: &str, contents: &str| {
        let path = directory.join("storage/message").join(relative);
        std::fs::create_dir_all(path.parent().expect("message file has a parent"))
            .expect("failed to create OpenCode message directory");
        std::fs::write(path, contents).expect("failed to write OpenCode message file");
    };
    let message = |id: &str, session: &str, body: &str| {
        format!(r#"{{"id":"{id}","sessionID":"{session}",{}"#, &body[1..])
    };

    let beta = "ses-json-beta";
    write(
        &format!("{beta}/msg-json-1.json"),
        &format!(
            r#"{{
  "id": "msg-json-1",
  "sessionID": "{beta}",
  "role": "assistant",
  "modelID": "{SONNET}",
  "providerID": "anthropic",
  "time": {{
    "created": {}
  }},
  "tokens": {{
    "input": 100,
    "output": 50,
    "cache": {{ "read": 0, "write": 0 }}
  }}
}}
"#,
            instant("2025-12-27T22:00:00Z").millis()
        ),
    );
    write(
        &format!("{beta}/msg-json-2.json"),
        &message(
            "msg-json-2",
            beta,
            &payload(
                &flat(HAIKU, "anthropic"),
                Some(instant("2025-12-31T23:50:00Z").millis()),
                &tokens(900, 400, 7000, 200),
                r#","cost":2.0"#,
            ),
        ),
    );
    write(
        "ses-agg-json/msg-json-agg.json",
        &message(
            "msg-json-agg",
            "ses-agg-json",
            &payload(
                &flat(SONNET, "anthropic"),
                Some(instant("2026-01-01T14:00:00Z").millis()),
                &tokens(80, 8, 0, 0),
                "",
            ),
        ),
    );
    write(
        "ses-json-loose/no-id.json",
        &payload(
            &flat("gpt-5", "openai"),
            Some(instant("2026-01-02T12:00:00Z").millis()),
            &tokens(13, 3, 0, 0),
            "",
        ),
    );

    let replay = |id: &str| {
        message(
            id,
            "ses-v1-alpha",
            &payload(
                &flat(SONNET, "anthropic"),
                Some(instant("2025-12-30T10:00:00Z").millis()),
                &tokens(9999, 9999, 0, 0),
                "",
            ),
        )
    };
    write("ses-v1-alpha/msg-v1-a1.json", &replay("msg-v1-a1"));
    write("ses-v1-alpha/copy-of-a2.json", &replay("msg-v1-a2"));

    write(&format!("{beta}/empty.json"), "");
    write(
        &format!("{beta}/truncated.json"),
        &format!(
            r#"{{"id":"msg-json-truncated","sessionID":"{beta}","modelID":"{SONNET}","providerID":"anthropic","tokens":{{"input":9999,"outp"#
        ),
    );
    set_modified(
        &directory.join(format!("storage/message/{beta}/truncated.json")),
        instant("2025-12-27T22:00:00Z"),
    );
    write(&format!("{beta}/array.json"), r#"["msg-json-1"]"#);
    write(
        &format!("{beta}/part.json"),
        r#"{"id":"prt-1","type":"step-finish","messageID":"msg-json-1","reason":"stop"}"#,
    );
    write(&format!("{beta}/notes.txt"), "not a message\n");
}

fn seed_claude_store(directory: &Path) {
    let project = directory.join("projects/-Users-dev-agent");
    std::fs::create_dir_all(&project).expect("failed to create Claude project directory");
    let transcript = project.join("claude-native-a.jsonl");
    std::fs::write(
        &transcript,
        format!(
            r#"{{"type":"assistant","timestamp":"2025-12-31T12:00:00.000Z","sessionId":"claude-native-a","version":"2.0.14","requestId":"req-native-1","message":{{"role":"assistant","id":"msg_native_1","model":"{OPUS}","usage":{{"input_tokens":5100,"output_tokens":510}}}}}}
"#
        ),
    )
    .expect("failed to write Claude transcript");
    set_modified(&transcript, instant("2026-01-03T00:00:00Z"));
}

pub(crate) fn seed(root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
    let store = root.join("opencode");
    let directory = |name: &str| {
        let path = store.join(name);
        std::fs::create_dir_all(&path).expect("failed to create OpenCode data directory");
        path
    };

    let main = directory("main");
    seed_main_database(&main.join("opencode.db"));
    seed_message_files(&main);

    let channel = directory("channel");
    seed_channel_database(&channel);

    let broken = directory("broken");
    std::fs::write(broken.join("opencode.db"), "not a sqlite database\n")
        .expect("failed to write broken OpenCode database");
    let broken_messages = broken.join("storage/message/ses-broken");
    std::fs::create_dir_all(&broken_messages).expect("failed to create OpenCode message directory");
    std::fs::write(
        broken_messages.join("msg-broken-1.json"),
        format!(
            r#"{{"id":"msg-broken-1","sessionID":"ses-broken",{}"#,
            &payload(
                &flat("gpt-5", "openai"),
                Some(instant("2025-12-30T12:00:00Z").millis()),
                &tokens(17, 7, 0, 0),
                "",
            )[1..]
        ),
    )
    .expect("failed to write OpenCode message file");

    let empty = directory("empty-db");
    std::fs::write(empty.join("opencode.db"), "").expect("failed to write empty OpenCode database");

    let claude = directory("claude");
    seed_claude_store(&claude);

    let data_dirs = [main, channel, broken, empty, store.join("missing")]
        .map(PathBuf::into_os_string)
        .to_vec();

    vec![
        ("OPENCODE_DATA_DIR", data_dirs),
        ("CLAUDE_CONFIG_DIR", vec![claude.into_os_string()]),
    ]
}

pub(crate) fn extra_cases(_root: &Path) -> Vec<Case> {
    vec![
        Case::new(
            "daily",
            "json_include_claude_agent",
            &[
                "opencode",
                "daily",
                "--offline",
                "--json",
                "--include-claude-agent",
            ],
        ),
        Case::new(
            "session",
            "json_include_claude_agent",
            &[
                "opencode",
                "session",
                "--offline",
                "--json",
                "--include-claude-agent",
            ],
        ),
        Case::new("daily", "table_warn", &["opencode", "daily", "--offline"]).env("LOG_LEVEL", "3"),
    ]
}
