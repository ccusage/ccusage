#![cfg(test)]

use ccusage_test_support::Fixture;

mod support;

#[test]
fn unified_calculate_uses_the_logged_host_listing_for_droid_and_hermes() {
    let fixture = Fixture::new();
    let root = fixture.root();
    let droid = root.join("droid");
    let hermes = root.join("hermes");
    std::fs::create_dir(&droid).unwrap();
    std::fs::create_dir(&hermes).unwrap();
    let model = "deepinfra/zai-org/GLM-4.7-Flash";
    std::fs::write(
        droid.join("test.settings.json"),
        serde_json::json!({
            "model": model,
            "providerLock": "deepinfra",
            "providerLockTimestamp": "2026-11-17T10:00:00Z",
            "tokenUsage": {"inputTokens": 1_000_000, "outputTokens": 1_000_000}
        })
        .to_string(),
    )
    .unwrap();
    sqlite::open(hermes.join("state.db"))
        .unwrap()
        .execute(format!(
            "CREATE TABLE sessions (id TEXT, model TEXT, billing_provider TEXT, started_at REAL,
             message_count INTEGER, input_tokens INTEGER, output_tokens INTEGER, cache_read_tokens INTEGER,
             cache_write_tokens INTEGER, reasoning_tokens INTEGER, estimated_cost_usd REAL, actual_cost_usd REAL);
             INSERT INTO sessions (id, model, started_at, message_count, input_tokens, output_tokens)
             VALUES ('test', '{model}', 1794909600, 1, 1000000, 1000000);"
        ))
        .unwrap();

    let output = support::hermetic_command(root)
        .env("DROID_SESSIONS_DIR", droid)
        .env("HERMES_HOME", hermes)
        .args([
            "daily",
            "--json",
            "--offline",
            "--mode",
            "calculate",
            "--by-agent",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let day = report["daily"].as_array().unwrap().first().unwrap();
    let rows = day["agents"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{report}");
    let total = day["totalCost"].as_f64().unwrap();
    for agent in ["droid", "hermes"] {
        let row = rows.iter().find(|row| row["agent"] == agent).unwrap();
        let cost = row["totalCost"].as_f64().unwrap();
        assert!(
            (cost - 0.46).abs() < 1e-12,
            "{agent} billed ${cost}: {report}"
        );
    }
    assert!((total - 0.92).abs() < 1e-12, "{report}");
}
