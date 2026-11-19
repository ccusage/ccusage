use std::path::{Path, PathBuf};

const SONNET: &str = "claude-sonnet-4-20250514";
const OPUS: &str = "claude-opus-4-20250514";
const HAIKU: &str = "claude-haiku-4-5-20251001";
const UNPRICED: &str = "hermes-mystery-1";

const SESSION_COLUMNS: &str = "id, source, model, billing_provider, started_at, message_count, \
     input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, \
     estimated_cost_usd, actual_cost_usd";

pub fn seed(root: &Path) -> Vec<(&'static str, PathBuf)> {
    let store = root.join("hermes");
    let home = |name: &str| {
        let path = store.join(name);
        std::fs::create_dir_all(&path).expect("failed to create Hermes home");
        path
    };

    let primary = home("primary");
    seed_primary(&primary.join("state.db"));
    let profile = home("profile-b");
    seed_profile(&profile.join("state.db"));
    let legacy = home("legacy");
    seed_legacy(&legacy.join("state.db"));

    let empty = home("empty");
    std::fs::write(empty.join("state.db"), "").expect("failed to write empty Hermes database");
    let corrupt = home("corrupt");
    std::fs::write(
        corrupt.join("state.db"),
        b"SQLite format 3\0 truncated before the first page ends",
    )
    .expect("failed to write corrupt Hermes database");
    let no_database = home("no-database");
    std::fs::write(
        no_database.join("sessions.json"),
        format!(
            r#"[{{"id":"json-session","model":"{SONNET}","started_at":1767180000,"input_tokens":5555,"output_tokens":5555}}]"#
        ),
    )
    .expect("failed to write stray Hermes file");
    let directory = home("directory");
    std::fs::create_dir_all(directory.join("state.db"))
        .expect("failed to create Hermes state.db directory");

    // HERMES_HOME is a comma-separated list. The primary home is listed twice
    // and padded with blanks and an empty segment, and the homes after it are
    // ones the loader has to skip without failing the report.
    let homes = [
        primary.display().to_string(),
        format!(" {} ", primary.display()),
        String::new(),
        profile.display().to_string(),
        legacy.display().to_string(),
        empty.display().to_string(),
        corrupt.display().to_string(),
        no_database.display().to_string(),
        directory.display().to_string(),
        store.join("absent").display().to_string(),
    ];
    vec![("HERMES_HOME", PathBuf::from(homes.join(",")))]
}

fn create(path: &Path, schema: &str, inserts: &[String]) {
    let db = sqlite::open(path).expect("failed to create Hermes state database");
    db.execute(schema)
        .expect("failed to create Hermes fixture schema");
    for insert in inserts {
        db.execute(insert)
            .expect("failed to insert Hermes fixture row");
    }
}

fn session(values: &str) -> String {
    format!("INSERT INTO sessions ({SESSION_COLUMNS}) VALUES ({values});")
}

/// Upstream's own schema, plus a column and a table the loader never reads.
fn seed_primary(path: &Path) {
    create(
        path,
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
                actual_cost_usd REAL,
                title TEXT
            );
            CREATE TABLE messages (
                id INTEGER PRIMARY KEY,
                session_id TEXT,
                model TEXT,
                input_tokens INTEGER,
                output_tokens INTEGER
            );
        ",
        &[
            // 2025-12-27T22:00:00Z, no stored cost at all.
            session(&format!(
                "'p-stale', 'cli', '{SONNET}', 'anthropic', 1766872800.0, 3, 100, 50, 0, 0, 0, NULL, NULL"
            )),
            // 2025-12-30T10:00:00.250Z, both costs stored and disagreeing with
            // each other and with the token-derived cost.
            session(&format!(
                "'p-sonnet-stored', 'cli', '{SONNET}', 'claude', 1767088800.25, 12, 1000, 200, 4000, 300, 0, 0.11, 0.5"
            )),
            // 2025-12-31T15:30:00Z (2026-01-01 in Asia/Tokyo), estimated cost only.
            session(
                "'p-gpt5-estimated', 'telegram', 'gpt-5', 'openai-codex', 1767195000.0, 8, 700, 150, 2500, 0, 60, 0.25, NULL",
            ),
            // 2025-12-31T23:50:00Z, a recorded zero cost and no provider.
            session(&format!(
                "'p-haiku-zero-cost', 'cli', '{HAIKU}', NULL, 1767225000.0, 5, 900, 400, 7000, 200, 0, NULL, 0.0"
            )),
            // 2026-01-01T00:10:00Z, padded model name and a negative estimate.
            session(&format!(
                "'p-opus-padded', 'cli', '  {OPUS}  ', ' Anthropic ', 1767226200.0, 4, 600, 100, 3000, 50, 0, -1.0, NULL"
            )),
            // 2026-01-01T12:00:00Z.
            session(&format!(
                "'p-mystery', 'cli', '{UNPRICED}', NULL, 1767268800.0, 2, 300, 70, 0, 0, 10, NULL, NULL"
            )),
            // 2026-01-02T10:00:00Z, a cost with no tokens.
            session(
                "'p-cost-only', 'cli', 'gpt-5', 'openai', 1767348000.0, 1, 0, 0, 0, 0, 0, 0.2, 0.3",
            ),
            // 2026-01-02T11:00:00Z, a model that only resolves with its provider prefix.
            session(
                "'p-openrouter', 'cli', 'openai/gpt-5', 'openrouter', 1767351600.0, 6, 800, 300, 6000, 0, 40, NULL, NULL",
            ),
            // Rows the loader must drop.
            session(
                "'p-no-model', 'cli', NULL, 'anthropic', 1767180000.0, 1, 9999, 9999, 0, 0, 0, NULL, 9.99",
            ),
            session(
                "'p-blank-model', 'cli', '   ', 'anthropic', 1767180000.0, 1, 9999, 9999, 0, 0, 0, NULL, 9.99",
            ),
            session(&format!(
                "'', 'cli', '{SONNET}', 'anthropic', 1767180000.0, 1, 9999, 9999, 0, 0, 0, NULL, 9.99"
            )),
            session(&format!(
                "'p-zero-time', 'cli', '{SONNET}', 'anthropic', 0.0, 1, 8888, 8888, 0, 0, 0, NULL, 8.88"
            )),
            session(&format!(
                "'p-negative-time', 'cli', '{SONNET}', 'anthropic', -1767180000.0, 1, 8888, 8888, 0, 0, 0, NULL, 8.88"
            )),
            session(&format!(
                "'p-no-usage', 'cli', '{SONNET}', 'anthropic', 1767180000.0, 7, 0, 0, 0, 0, 0, 0.0, 0.0"
            )),
            format!(
                "INSERT INTO messages (session_id, model, input_tokens, output_tokens) \
                 VALUES ('p-stale', '{SONNET}', 5555, 5555);"
            ),
        ],
    );
}

/// A second home whose table has no primary key or column types, so it can hold
/// what upstream's schema forbids: repeated ids and values of the wrong type.
fn seed_profile(path: &Path) {
    create(
        path,
        "
            CREATE TABLE sessions (
                id, source, model, billing_provider, started_at, message_count,
                input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                reasoning_tokens, estimated_cost_usd, actual_cost_usd
            );
        ",
        &[
            // Same id as a session of the primary home: the earlier home wins.
            session(&format!(
                "'p-sonnet-stored', 'cli', '{SONNET}', 'claude', 1767180000.0, 99, 9999, 9999, 9999, 9999, 9999, 9.99, 9.99"
            )),
            // Same content as that session under another id: counted.
            session(&format!(
                "'p-sonnet-stored-2', 'cli', '{SONNET}', 'claude', 1767088800.25, 12, 1000, 200, 4000, 300, 0, 0.11, 0.5"
            )),
            // 2025-12-31T16:00:00Z as integer milliseconds; the id repeats in
            // the same table and only the first row counts.
            session(&format!(
                "'b-twin', 'cli', '{SONNET}', 'anthropic', 1767196800000, 2, 400, 80, 0, 0, 0, NULL, NULL"
            )),
            session(&format!(
                "'b-twin', 'cli', '{SONNET}', 'anthropic', 1767200400000, 77, 7777, 7777, 7777, 7777, 7777, 7.77, 7.77"
            )),
            // 2025-12-31T03:00:00Z, the same day in Asia/Tokyo.
            session(&format!(
                "'b-morning', 'cli', '{HAIKU}', 'anthropic', 1767150000.0, 3, 250, 40, 500, 20, 0, NULL, NULL"
            )),
            // 2026-01-01T09:00:00Z as integer seconds, with real, text and
            // negative token counts and an unknown provider.
            session(
                "'b-odd-types', 'cli', 'gpt-5', 'Weird-Provider', 1767258000, '3', 120.9, 30.2, '45', -5, 'many', 'free', NULL",
            ),
            // 2026-01-02T08:00:00Z, a provider with its own pricing entries.
            session(
                "'b-deepseek', 'cli', 'deepseek-v4-flash', 'deepseek', 1767340800.0, 2, 500, 100, 1000, 0, 0, NULL, NULL",
            ),
            session(&format!(
                "'b-null-time', 'cli', '{SONNET}', 'anthropic', NULL, 1, 8888, 8888, 0, 0, 0, NULL, 8.88"
            )),
            session(&format!(
                "'b-text-time', 'cli', '{SONNET}', 'anthropic', 'yesterday', 1, 6666, 6666, 0, 0, 0, NULL, 6.66"
            )),
        ],
    );
}

/// An older sessions table without the cache, reasoning and cost columns.
fn seed_legacy(path: &Path) {
    create(
        path,
        "
            CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                source TEXT NOT NULL,
                model TEXT,
                started_at REAL NOT NULL,
                message_count INTEGER DEFAULT 0,
                input_tokens INTEGER DEFAULT 0,
                output_tokens INTEGER DEFAULT 0
            );
        ",
        &[format!(
            "INSERT INTO sessions (id, source, model, started_at, message_count, input_tokens, output_tokens) \
             VALUES ('legacy-session', 'cli', '{SONNET}', 1767180000.0, 4, 7777, 7777);"
        )],
    );
}
