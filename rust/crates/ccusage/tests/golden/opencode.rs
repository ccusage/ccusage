use std::ffi::OsString;
use std::path::{Path, PathBuf};

use ccusage_test_support::{instant, set_modified};

use crate::golden::Case;

const SONNET: &str = "claude-sonnet-4-20250514";
const OPUS: &str = "claude-opus-4-20250514";
const HAIKU: &str = "claude-haiku-4-5-20251001";
const UNPRICED: &str = "opencode-mystery-1";

/// The v1 and legacy-JSON model reference: flat members on the message.
fn flat(model: &str, provider: &str) -> String {
    format!(r#""modelID":"{model}","providerID":"{provider}""#)
}

/// The v2 model reference: a nested `model` object.
fn nested(model: &str, provider: &str) -> String {
    format!(r#""model":{{"id":"{model}","providerID":"{provider}"}}"#)
}

fn tokens(input: u64, output: u64, cache_read: u64, cache_write: u64) -> String {
    format!(
        r#"{{"input":{input},"output":{output},"cache":{{"read":{cache_read},"write":{cache_write}}}}}"#
    )
}

/// One assistant payload. `created` is the payload's own `time.created`, and
/// `extra` holds further top-level members, each with its leading comma.
fn payload(model: &str, created: Option<i64>, tokens: &str, extra: &str) -> String {
    let time = created.map_or_else(String::new, |created| {
        format!(r#""time":{{"created":{created}}},"#)
    });
    format!(r#"{{"role":"assistant",{model},{time}"tokens":{tokens}{extra}}}"#)
}

/// A `claude-agent` v2 payload linked to the Claude Code session it ran in.
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

    /// A v1 `message` row.
    fn message(&self, id: &str, session: &str, created: i64, data: &str) {
        self.execute(&format!(
            "INSERT INTO message (id, session_id, time_created, data) VALUES ({}, {}, {created}, {})",
            sql_text(id),
            sql_text(session),
            sql_text(data),
        ));
    }

    /// A pre-`time_created` `message` row, in a table without a primary key.
    fn legacy_message(&self, id: &str, session: &str, data: &str) {
        self.execute(&format!(
            "INSERT INTO message (id, session_id, data) VALUES ({}, {}, {})",
            sql_text(id),
            sql_text(session),
            sql_text(data),
        ));
    }

    /// A v2 `session_message` row.
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

    /// Cumulative session counters in `session_v2` or the older `session`.
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

/// The primary data directory: one database holding every table generation
/// (v1 `message`, v2 `session_message`, `session_v2`, and the older `session`).
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
            // A table the loader has no use for.
            "CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, data TEXT)",
        ],
    );
    db.execute(
        r#"INSERT INTO part VALUES ('part-1', 'msg-v1-a1', '{"type":"step-finish","tokens":{"input":9999,"output":9999}}')"#,
    );

    // --- v1 `message` rows -------------------------------------------------
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
    // 2026-01-01 in Asia/Tokyo; reasoning tokens are counted on top.
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
    // Aliased model ID, resolved through the adapter's mapping table.
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
    // Only a total: the remainder becomes output. The dotted model name is
    // normalized for pricing, and a zero stored cost counts as absent.
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
    // Rows that must not be counted.
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

    // --- v2 `session_message` rows -----------------------------------------
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
    // The fork boundary; 2026-01-01 in Asia/Tokyo.
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
    // Flat model members on a v2 row.
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
    // Same id as a v1 row: the v1 row wins.
    db.session_message(
        "msg-v1-a1",
        alpha,
        "assistant",
        0,
        Some(instant("2025-12-30T10:00:00Z").millis()),
        &turn(9999, 9999, instant("2025-12-30T10:00:00Z").millis(), ""),
    );

    // Forks copy the parent's rows under new ids but the same `seq`. The
    // copies carry 9xxx tokens here so a leak is visible.
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
    // A `before` fork stops one row short of the boundary, so its own row at
    // the boundary's `seq` is counted.
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
    // The boundary names a message the parent no longer has: every row stays.
    let lost = "ses-v2-fork-lost";
    db.session_message(
        "msg-v2-fl-kept",
        lost,
        "assistant",
        0,
        Some(instant("2026-01-01T10:00:00Z").millis()),
        &turn(21, 2, instant("2026-01-01T10:00:00Z").millis(), ""),
    );
    // Only copies: the fork-local `session_v2` counters are the fallback.
    let idle = "ses-v2-fork-idle";
    copy("msg-v2-fi-copy-1", idle, 1, 9006);

    // Compaction is the session's only row, so session reports use the
    // counters (which include it) and the other reports use the row.
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

    // No `time.created` in the payload: the column is the timestamp, and a
    // row whose column is NULL is skipped as an invalid timestamp.
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

    // Claude Agent rows. The Claude store written by `seed_claude_store`
    // counts `claude-native-a` on 2025-12-31 only.
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

    // --- Session counters (session reports only) ---------------------------
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
    // Already counted from `session_v2`, and covered by v1 rows.
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

/// A release-channel database in the pre-`time_created` schema, in a second
/// data directory.
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
    // The same id twice in a table without a primary key: the first wins.
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
    // Already loaded from the primary directory.
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
    // Covers a session whose counters live in the primary directory.
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

    // Only the first channel database by name is read.
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

/// Legacy `storage/message/<session>/<message>.json` files beside the primary
/// database.
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
    // The stale day, pretty-printed as OpenCode wrote these files.
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
    // 2026-01-01 in Asia/Tokyo, with a stored cost.
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
    // Covers a session that also has `session_v2` counters.
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
    // Neither an id nor a session: never deduped, session "unknown".
    write(
        "ses-json-loose/no-id.json",
        &payload(
            &flat("gpt-5", "openai"),
            Some(instant("2026-01-02T12:00:00Z").millis()),
            &tokens(13, 3, 0, 0),
            "",
        ),
    );

    // Already loaded from the database: skipped by file stem, and by the id
    // inside the file when the stem differs.
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

/// The Claude Code transcript behind the `claude-agent` rows. `ccusage
/// opencode` reads it to leave the turns Claude already counts to Claude.
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

pub fn seed(root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
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

    // A database that is not SQLite does not hide the message files beside it.
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

    // `OPENCODE_DATA_DIR` is a comma-separated list; the last entry does not
    // exist and is skipped.
    let data_dirs = [main, channel, broken, empty, store.join("missing")]
        .map(PathBuf::into_os_string)
        .to_vec();

    vec![
        ("OPENCODE_DATA_DIR", data_dirs),
        ("CLAUDE_CONFIG_DIR", vec![claude.into_os_string()]),
    ]
}

pub fn extra_cases(_root: &Path) -> Vec<Case> {
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
