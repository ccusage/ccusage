use std::path::Path;

use ccusage_test_support::Fixture;
use serde_json::Value;

fn metadata_blob(response_id: &str, input_tokens: u8) -> Vec<u8> {
    let mut usage = vec![
        0x10,
        input_tokens,
        0x18,
        5,
        0x50,
        5,
        0x5a,
        response_id.len() as u8,
    ];
    usage.extend_from_slice(response_id.as_bytes());

    let mut chat_model = vec![0x22, usage.len() as u8];
    chat_model.extend(usage);
    chat_model.extend([0x9a, 0x01, 12]);
    chat_model.extend_from_slice(b"gemini-3-pro");

    let mut metadata = vec![0x0a, chat_model.len() as u8];
    metadata.extend(chat_model);
    metadata
}

fn create_database(path: &Path, rows: &[&[u8]]) {
    let connection = sqlite::open(path).unwrap();
    connection
        .execute("CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB NOT NULL);")
        .unwrap();
    connection
        .execute("CREATE TABLE steps (idx INTEGER PRIMARY KEY, metadata BLOB);")
        .unwrap();
    connection
        .execute("CREATE TABLE trajectory_metadata_blob (data BLOB);")
        .unwrap();

    let mut statement = connection
        .prepare("INSERT INTO gen_metadata (idx, data) VALUES (?1, ?2)")
        .unwrap();
    for (index, data) in rows.iter().enumerate() {
        statement.bind((1, index as i64 + 1)).unwrap();
        statement.bind((2, *data)).unwrap();
        statement.next().unwrap();
        statement.reset().unwrap();
    }
}

#[test]
fn antigravity_skips_corrupt_databases_and_warns_without_dropping_other_reports() {
    let fixture = Fixture::new();
    let root = fixture.path("antigravity");
    let conversations = root.join("conversations");
    let _ = fixture.create_dir_all("antigravity/conversations");
    create_database(
        &conversations.join("healthy.db"),
        &[&metadata_blob("healthy-response", 10)],
    );
    create_database(
        &conversations.join("partial.db"),
        &[
            &metadata_blob("must-not-be-counted", 100),
            &[0x0a, 0x01, 0x80],
        ],
    );
    let _ = fixture.write_file("antigravity/conversations/not-a-database.db", "not sqlite");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ccusage"))
        .env_clear()
        .env("HOME", fixture.path("home"))
        .env("USERPROFILE", fixture.path("userprofile"))
        .env("XDG_CONFIG_HOME", fixture.path("xdg-config"))
        .env("ANTIGRAVITY_DATA_DIR", &root)
        .args([
            "antigravity",
            "session",
            "--json",
            "--offline",
            "--timezone",
            "UTC",
        ])
        .output()
        .expect("failed to run ccusage");

    assert!(
        output.status.success(),
        "ccusage antigravity failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).expect("CLI stdout was not JSON");
    assert_eq!(report["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(report["sessions"][0]["sessionId"], "healthy");
    assert_eq!(report["sessions"][0]["inputTokens"], 10);
    assert_eq!(report["sessions"][0]["outputTokens"], 5);

    let stderr = String::from_utf8(output.stderr).expect("CLI stderr was not UTF-8");
    assert!(
        stderr.contains("WARN"),
        "expected a warning in stderr: {stderr}"
    );
    assert!(
        stderr.contains("partial.db"),
        "expected the skipped database filename in stderr: {stderr}"
    );
    assert!(
        stderr.contains("not-a-database.db"),
        "expected the skipped database filename in stderr: {stderr}"
    );
}
