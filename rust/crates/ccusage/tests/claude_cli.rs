#![cfg(test)]

use std::time::Duration;

use ccusage_test_support::{Fixture, Instant, instant, set_modified};

mod support;

fn claude_command(fixture: &Fixture) -> std::process::Command {
    let mut command = support::hermetic_command(fixture.root());
    command.env("CLAUDE_CONFIG_DIR", fixture.root());
    command
}

#[test]
fn calculate_mode_uses_primary_mythos_prices_from_the_embedded_snapshot() {
    for (model, expected) in [
        ("claude-mythos-5-1", 0.002175),
        ("claude-mythos-5-2", 0.00174),
        ("claude-mythos-preview", 0.002325),
    ] {
        let fixture = Fixture::new();
        fixture.write_file(
            "projects/project-a/session-a.jsonl",
            serde_json::json!({
                "timestamp": "2026-09-15T12:00:00Z",
                "sessionId": "session-a",
                "message": {
                    "id": "message-a",
                    "model": model,
                    "usage": {
                        "input_tokens": 100,
                        "output_tokens": 20,
                        "cache_creation_input_tokens": 10,
                        "cache_read_input_tokens": 200,
                    },
                },
            })
            .to_string(),
        );
        let output = claude_command(&fixture)
            .args([
                "claude",
                "session",
                "--json",
                "--mode",
                "calculate",
                "--offline",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}: {}",
            model,
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let cost = report["sessions"][0]["totalCost"].as_f64().unwrap();
        assert!((cost - expected).abs() < 1e-12, "{model}: {cost}");
    }
}

#[test]
fn session_id_deduplicates_repeated_message_usage() {
    let fixture = Fixture::new();
    let messages = [
        r#"{"timestamp":"2026-09-15T12:00:00.000Z","sessionId":"session-a","requestId":"request-a","costUSD":1.25,"message":{"id":"message-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":2}}}"#,
        r#"{"timestamp":"2026-09-15T12:00:01.000Z","sessionId":"session-a","requestId":"request-a","costUSD":1.25,"message":{"id":"message-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":2}}}"#,
        r#"{"timestamp":"2026-09-15T12:00:02.000Z","sessionId":"session-a","requestId":"request-a","costUSD":1.25,"message":{"id":"message-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":2}}}"#,
    ];
    fixture.write_file(
        "projects/project-a/session-a/chat.jsonl",
        messages.join("\n"),
    );

    let output = claude_command(&fixture)
        .args([
            "session",
            "--id",
            "session-a",
            "--json",
            "--mode",
            "display",
        ])
        .output()
        .expect("failed to run ccusage");

    assert!(
        output.status.success(),
        "ccusage session --id failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["totalCost"], 1.25);
    assert_eq!(json["totalTokens"], 12);
    assert_eq!(json["entries"].as_array().unwrap().len(), 1);
}

#[test]
fn session_counts_usage_written_with_json_whitespace() {
    let fixture = Fixture::new();
    fixture.write_file(
        "projects/project-a/session-a/chat.jsonl",
        r#"{"type":"assistant","timestamp":"2026-09-11T12:00:00.000Z","sessionId":"session-a","message":{"id":"m1","model":"claude-sonnet-4-20250514","usage": {"input_tokens":100,"output_tokens":2}}}"#,
    );

    let output = claude_command(&fixture)
        .args([
            "claude",
            "session",
            "--offline",
            "--json",
            "--mode",
            "display",
        ])
        .output()
        .expect("failed to run ccusage");

    assert!(
        output.status.success(),
        "ccusage claude session --offline failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["sessions"][0]["inputTokens"], 100);
    assert_eq!(json["sessions"][0]["outputTokens"], 2);
    assert_eq!(json["totals"]["totalTokens"], 102);
}

#[test]
fn session_id_keeps_tied_timestamps_in_input_order_for_both_orders() {
    let fixture = Fixture::new();
    let lines = [
        ("2026-09-15T12:01:00.000Z", 1),
        ("2026-09-15T12:00:00.000Z", 2),
        ("2026-09-15T12:01:00.000Z", 3),
        ("2026-09-15T12:00:00.000Z", 4),
        ("2026-09-15T12:02:00.000Z", 5),
        ("2026-09-15T12:01:00.000Z", 6),
    ]
    .map(|(timestamp, input)| {
        format!(
            r#"{{"timestamp":"{timestamp}","sessionId":"session-a","requestId":"request-{input}","costUSD":0.5,"message":{{"id":"message-{input}","model":"claude-sonnet-4-20250514","usage":{{"input_tokens":{input},"output_tokens":1}}}}}}"#
        )
    });
    fixture.write_file("projects/project-a/session-a/chat.jsonl", lines.join("\n"));

    let entry_inputs = |order: &str| {
        let json: serde_json::Value = serde_json::from_str(&run_ccusage(
            &fixture,
            &[
                "claude",
                "session",
                "--id",
                "session-a",
                "--json",
                "--offline",
                "--order",
                order,
            ],
            None,
        ))
        .unwrap();
        json["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["inputTokens"].as_u64().unwrap())
            .collect::<Vec<_>>()
    };

    assert_eq!(entry_inputs("asc"), [2, 4, 1, 3, 6, 5]);
    assert_eq!(entry_inputs("desc"), [5, 1, 3, 6, 2, 4]);
}

fn write_two_model_session(fixture: &Fixture) {
    let messages = [
        r#"{"timestamp":"2026-09-15T12:00:00.000Z","sessionId":"session-a","requestId":"request-a","costUSD":0.25,"message":{"id":"message-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":2}}}"#,
        r#"{"timestamp":"2026-09-15T12:01:00.000Z","sessionId":"session-a","requestId":"request-b","costUSD":1.5,"message":{"id":"message-b","model":"claude-opus-4-20250514","usage":{"input_tokens":30,"output_tokens":4,"cache_read_input_tokens":100}}}"#,
    ];
    fixture.write_file(
        "projects/project-a/session-a/chat.jsonl",
        messages.join("\n"),
    );
    fixture.write_file(
        "projects/project-b/session-b/chat.jsonl",
        r#"{"timestamp":"2026-09-15T13:00:00.000Z","sessionId":"session-b","requestId":"request-c","costUSD":9.0,"message":{"id":"message-c","model":"claude-sonnet-4-20250514","usage":{"input_tokens":999,"output_tokens":999}}}"#,
    );
}

#[test]
fn session_id_json_includes_model_breakdowns() {
    let fixture = Fixture::new();
    write_two_model_session(&fixture);

    let json: serde_json::Value = serde_json::from_str(&run_ccusage(
        &fixture,
        &[
            "claude",
            "session",
            "--id",
            "session-a",
            "--json",
            "--mode",
            "display",
            "--offline",
        ],
        None,
    ))
    .unwrap();

    assert_eq!(
        json["modelsUsed"],
        serde_json::json!(["claude-sonnet-4-20250514", "claude-opus-4-20250514"])
    );
    let breakdowns = json["modelBreakdowns"].as_array().unwrap();
    assert_eq!(breakdowns.len(), 2, "{json}");
    assert_eq!(breakdowns[0]["modelName"], "claude-opus-4-20250514");
    assert_eq!(breakdowns[0]["inputTokens"], 30);
    assert_eq!(breakdowns[0]["outputTokens"], 4);
    assert_eq!(breakdowns[0]["cacheReadTokens"], 100);
    assert_eq!(breakdowns[0]["cost"], 1.5);
    assert_eq!(breakdowns[1]["modelName"], "claude-sonnet-4-20250514");
    assert_eq!(breakdowns[1]["inputTokens"], 10);
    assert_eq!(breakdowns[1]["cost"], 0.25);
}

#[test]
fn session_id_table_honors_breakdown() {
    let fixture = Fixture::new();
    write_two_model_session(&fixture);

    let output = claude_command(&fixture)
        .env("COLUMNS", "200")
        .args([
            "claude",
            "session",
            "--id",
            "session-a",
            "--breakdown",
            "--mode",
            "display",
            "--offline",
            "--timezone",
            "UTC",
        ])
        .output()
        .expect("failed to run ccusage");
    assert!(
        output.status.success(),
        "ccusage claude session --id --breakdown failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    insta::assert_snapshot!(String::from_utf8(output.stdout).unwrap());
}

fn run_bounded_daily(fixture: &Fixture, extra_args: &[&str]) -> std::process::Output {
    let output = claude_command(fixture)
        .args([
            "claude",
            "daily",
            "--since",
            "20260910",
            "--timezone",
            "UTC",
            "--mode",
            "display",
            "--offline",
        ])
        .args(extra_args)
        .output()
        .expect("failed to run ccusage");
    assert!(
        output.status.success(),
        "ccusage claude daily failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn daily_since_totals_match_after_pruning_stale_files() {
    let fixture = Fixture::new();
    let entry = |timestamp: &str, id: &str, cost: f64| {
        format!(
            r#"{{"timestamp":"{timestamp}","sessionId":"{id}","requestId":"request-{id}","costUSD":{cost},"message":{{"id":"message-{id}","model":"claude-sonnet-4-20250514","usage":{{"input_tokens":10,"output_tokens":2}}}}}}"#
        )
    };

    let stale = fixture.write_file(
        "projects/project-a/stale.jsonl",
        entry("2026-09-01T12:00:00.000Z", "stale", 4.0),
    );
    let margin = fixture.write_file(
        "projects/project-a/margin.jsonl",
        entry("2026-09-10T08:00:00.000Z", "margin", 2.0),
    );
    let fresh = fixture.write_file(
        "projects/project-a/fresh.jsonl",
        [
            entry("2026-09-09T23:00:00.000Z", "fresh-before", 8.0),
            entry("2026-09-11T12:00:00.000Z", "fresh", 1.0),
        ]
        .join("\n"),
    );

    set_modified(&stale, instant("2026-09-01T12:00:00Z"));
    set_modified(&margin, instant("2026-09-09T06:00:00Z"));
    set_modified(&fresh, instant("2026-09-11T12:00:00Z"));

    let json_output = run_bounded_daily(&fixture, &["--json"]);
    let json: serde_json::Value = serde_json::from_slice(&json_output.stdout).unwrap();
    let days = json["daily"]
        .as_array()
        .unwrap()
        .iter()
        .map(|day| {
            (
                day["date"].as_str().unwrap().to_string(),
                day["totalCost"].as_f64().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        days,
        vec![
            ("2026-09-10".to_string(), 2.0),
            ("2026-09-11".to_string(), 1.0)
        ]
    );
    assert_eq!(json["totals"]["totalCost"], 3.0);
    assert_eq!(json["totals"]["totalTokens"], 24);

    let table_output = run_bounded_daily(&fixture, &[]);
    let table = String::from_utf8_lossy(&table_output.stdout);
    assert!(table.contains("2026-09-10"), "{table}");
    assert!(table.contains("2026-09-11"), "{table}");
    assert!(!table.contains("2026-09-01"), "{table}");
    assert!(!table.contains("2026-09-09"), "{table}");
    assert!(table.contains("$3.00"), "{table}");
}

#[test]
fn daily_uses_the_process_timezone_for_default_and_local_buckets() {
    let fixture = Fixture::new();
    fixture.write_file(
        "projects/project-a/session-a.jsonl",
        r#"{"timestamp":"2026-09-11T23:30:00Z","sessionId":"session-a","message":{"id":"message-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":1,"output_tokens":2}}}"#,
    );

    for (timezone, expected_date) in [
        (&[][..], "2026-09-12"),
        (&["--timezone", "local"][..], "2026-09-12"),
        (&["--timezone", "UTC"][..], "2026-09-11"),
    ] {
        let output = claude_command(&fixture)
            .env("TZ", "Asia/Tokyo")
            .args(["claude", "daily", "--offline", "--json"])
            .args(timezone)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let days = json["daily"].as_array().unwrap();
        assert_eq!(days.len(), 1);
        assert_eq!(days[0]["date"], expected_date, "{timezone:?}");
    }
}

#[test]
fn daily_reads_a_mixed_shape_line_as_its_direct_entry_and_unwraps_agent_progress() {
    let fixture = Fixture::new();
    fixture.write_file(
        "projects/project-a/session-a/chat.jsonl",
        [
            r#"{"timestamp":"2026-09-11T12:00:00.000Z","sessionId":"session-a","requestId":"request-direct","costUSD":1.0,"message":{"id":"message-direct","model":"claude-sonnet-4-20250514","usage":{"input_tokens":1,"output_tokens":2}},"data":{"message":{"timestamp":"2026-09-12T12:00:00.000Z","requestId":"request-shadow","costUSD":5.0,"message":{"id":"message-shadow","model":"claude-sonnet-4-20250514","usage":{"input_tokens":100,"output_tokens":200}}}}}"#,
            r#"{"type":"progress","sessionId":"session-a","data":{"message":{"timestamp":"2026-09-12T12:00:00.000Z","requestId":"request-wrapped","costUSD":2.0,"message":{"id":"message-wrapped","model":"claude-sonnet-4-20250514","usage":{"input_tokens":3,"output_tokens":4}}}}}"#,
        ]
        .join("\n"),
    );

    let output = run_bounded_daily(&fixture, &["--json"]);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let days = json["daily"]
        .as_array()
        .unwrap()
        .iter()
        .map(|day| {
            (
                day["date"].as_str().unwrap().to_string(),
                day["totalTokens"].as_u64().unwrap(),
                day["totalCost"].as_f64().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        days,
        vec![
            ("2026-09-11".to_string(), 3, 1.0),
            ("2026-09-12".to_string(), 7, 2.0),
        ]
    );
    assert_eq!(json["totals"]["totalTokens"], 10);
    assert_eq!(json["totals"]["totalCost"], 3.0);
}

#[cfg(unix)]
#[test]
fn claude_daily_counts_a_symlinked_project_directory() {
    let fixture = Fixture::new();
    fixture.write_file(
        "transcripts/session-a.jsonl",
        usage_line(
            "2026-09-11T12:00:00Z",
            "session-a",
            "message-a",
            "request-a",
            1.0,
        ),
    );
    fixture.create_dir_all("projects");
    std::os::unix::fs::symlink(
        fixture.path("transcripts"),
        fixture.path("projects/project-a"),
    )
    .unwrap();

    let output = run_ccusage(&fixture, &["claude", "daily", "--offline", "--json"], None);
    let report: serde_json::Value = serde_json::from_str(&output).unwrap();

    assert_eq!(report["totals"]["totalTokens"], 12);
}

fn run_ccusage(fixture: &Fixture, args: &[&str], stdin: Option<&str>) -> String {
    run_command(claude_command(fixture), args, stdin)
}

fn run_command(command: std::process::Command, args: &[&str], stdin: Option<&str>) -> String {
    String::from_utf8(run_command_output(command, args, stdin).stdout)
        .expect("ccusage stdout is UTF-8")
}

fn run_ccusage_output(
    fixture: &Fixture,
    args: &[&str],
    stdin: Option<&str>,
) -> std::process::Output {
    run_command_output(claude_command(fixture), args, stdin)
}

fn run_command_output(
    mut command: std::process::Command,
    args: &[&str],
    stdin: Option<&str>,
) -> std::process::Output {
    use std::io::Write as _;

    let mut child = command
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to run ccusage");
    let mut child_stdin = child.stdin.take().expect("piped ccusage stdin");
    child_stdin
        .write_all(stdin.unwrap_or_default().as_bytes())
        .expect("write ccusage input");
    drop(child_stdin);
    let output = child.wait_with_output().expect("wait for ccusage output");
    assert!(
        output.status.success(),
        "ccusage {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn statusline_emits_one_warning_across_repeated_usage_loads() {
    let fixture = Fixture::new();
    let transcript = fixture.write_file(
        "projects/project-a/session-now.jsonl",
        concat!(r#"{"message":{"usage":{"SECRET_CANARY":"#, "\n"),
    );
    let hook = serde_json::json!({
        "session_id": "session-now",
        "transcript_path": transcript,
        "model": {"display_name": "Model"},
    })
    .to_string();
    for cost_source in ["ccusage", "cc"] {
        let output = run_ccusage_output(
            &fixture,
            &[
                "claude",
                "statusline",
                "--offline",
                "--no-cache",
                "--cost-source",
                cost_source,
                "--timezone",
                "UTC",
            ],
            Some(&hook),
        );
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(
            stderr
                .lines()
                .filter(|line| line.contains("invalid data"))
                .count(),
            1,
            "{stderr}"
        );
        assert!(stderr.contains("skipped 1 record in 1 input"), "{stderr}");
        assert!(!stderr.contains("SECRET_CANARY"));
        assert!(
            String::from_utf8(output.stdout)
                .unwrap()
                .contains("session")
        );
    }
}

#[test]
fn a_transcript_line_left_cut_short_is_warned_about_however_recently_it_was_written() {
    let fixture = Fixture::new();
    let transcript = fixture.write_file(
        "projects/project-a/session-now.jsonl",
        format!(
            "{}\n{}",
            usage_line(
                "2026-01-02T00:00:00.000Z",
                "session-now",
                "msg-a",
                "req-a",
                0.5
            ),
            r#"{"timestamp":"2026-01-02T00:00:01.000Z","message":{"usage":{"input_tok"#
        ),
    );
    let hook = serde_json::json!({
        "session_id": "session-now",
        "transcript_path": transcript,
        "model": {"display_name": "Model"},
    })
    .to_string();

    let daily = run_ccusage_output(
        &fixture,
        &[
            "claude",
            "daily",
            "--offline",
            "--json",
            "--timezone",
            "UTC",
        ],
        None,
    );
    let statusline = run_ccusage_output(
        &fixture,
        &[
            "claude",
            "statusline",
            "--offline",
            "--no-cache",
            "--cost-source",
            "ccusage",
            "--timezone",
            "UTC",
        ],
        Some(&hook),
    );

    let report: serde_json::Value = serde_json::from_slice(&daily.stdout).unwrap();
    assert_eq!(report["totals"]["totalCost"], 0.5);
    for output in [daily, statusline] {
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains("WARN  claude: skipped 1 record in 1 input (invalid data)"),
            "{stderr}"
        );
    }
}

#[test]
fn statusline_debug_includes_sanitized_skip_samples() {
    let fixture = Fixture::new();
    let transcript = fixture.write_file(
        "projects/project-a/session-now.jsonl",
        concat!(r#"{"message":{"usage":{"SECRET_CANARY":"#, "\n"),
    );
    let hook = serde_json::json!({
        "session_id": "session-now",
        "transcript_path": transcript,
        "model": {"display_name": "Model"},
    })
    .to_string();
    let output = run_ccusage_output(
        &fixture,
        &[
            "claude",
            "statusline",
            "--offline",
            "--no-cache",
            "--cost-source",
            "cc",
            "--debug",
            "--timezone",
            "UTC",
        ],
        Some(&hook),
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("session-now.jsonl:1"), "{stderr}");
    assert!(stderr.contains("skipped 1 record in 1 input"), "{stderr}");
    assert!(!stderr.contains("SECRET_CANARY"), "{stderr}");
}

fn assert_output_ignores_mtimes(
    command: impl Fn() -> std::process::Command,
    files: &[std::path::PathBuf],
    fresh_mtime: Instant,
    args: &[&str],
    stdin: Option<&str>,
) -> String {
    let stale = run_command(command(), args, stdin);
    for file in files {
        set_modified(file, fresh_mtime);
    }
    let fresh = run_command(command(), args, stdin);
    assert_eq!(stale, fresh, "ccusage {args:?} depends on file mtimes");
    fresh
}

fn usage_line(timestamp: &str, session: &str, message: &str, request: &str, cost: f64) -> String {
    format!(
        r#"{{"timestamp":"{timestamp}","sessionId":"{session}","requestId":"{request}","costUSD":{cost},"message":{{"id":"{message}","model":"claude-sonnet-4-20250514","usage":{{"input_tokens":10,"output_tokens":2}}}}}}"#
    )
}

fn sidechain_line(
    timestamp: &str,
    session: &str,
    message: &str,
    request: &str,
    cost: f64,
) -> String {
    usage_line(timestamp, session, message, request, cost).replacen(
        '{',
        r#"{"isSidechain":true,"#,
        1,
    )
}

#[test]
fn bounded_reports_dedupe_sidechain_replays_of_parents_in_stale_files() {
    let fixture = Fixture::new();
    let parent = fixture.write_file(
        "projects/project-a/session-a.jsonl",
        usage_line(
            "2026-09-01T12:00:00.000Z",
            "session-a",
            "msg-parent",
            "req-parent",
            1.0,
        ),
    );
    let sidechain = fixture.write_file(
        "projects/project-a/session-a/subagents/agent-a.jsonl",
        [
            sidechain_line(
                "2026-09-11T12:00:00.000Z",
                "session-a",
                "msg-parent",
                "req-replay",
                2.0,
            ),
            sidechain_line(
                "2026-09-11T12:05:00.000Z",
                "session-a",
                "msg-answer",
                "req-answer",
                1.0,
            ),
        ]
        .join("\n"),
    );
    let files = [parent.clone(), sidechain.clone()];

    for command in ["daily", "weekly", "monthly", "session"] {
        set_modified(&parent, instant("2026-09-01T12:00:00Z"));
        set_modified(&sidechain, instant("2026-09-11T12:05:00Z"));
        let output = assert_output_ignores_mtimes(
            || claude_command(&fixture),
            &files,
            instant(NOW),
            &[
                "claude",
                command,
                "--since",
                "20260910",
                "--timezone",
                "UTC",
                "--mode",
                "display",
                "--offline",
                "--json",
            ],
            None,
        );
        if command == "daily" {
            let json: serde_json::Value = serde_json::from_str(&output).unwrap();
            assert_eq!(json["totals"]["totalCost"], 1.0, "{output}");
        }
    }
}

#[test]
fn bounded_unified_sessions_count_only_in_window_usage_of_old_subagent_files() {
    let fixture = Fixture::new();
    let parent = fixture.write_file(
        "projects/project-a/session-b.jsonl",
        usage_line(
            "2026-09-11T12:00:00.000Z",
            "session-b",
            "msg-recent",
            "req-recent",
            1.0,
        ),
    );
    let subagent = fixture.write_file(
        "projects/project-a/session-b/subagents/agent-b.jsonl",
        sidechain_line(
            "2026-09-01T12:00:00.000Z",
            "session-b",
            "msg-old",
            "req-old",
            2.0,
        ),
    );

    set_modified(&parent, instant("2026-09-11T12:00:00Z"));
    set_modified(&subagent, instant("2026-09-01T12:00:00Z"));

    let output = assert_output_ignores_mtimes(
        || claude_command(&fixture),
        &[parent, subagent],
        instant(NOW),
        &[
            "session",
            "--since",
            "20260910",
            "--timezone",
            "UTC",
            "--mode",
            "display",
            "--offline",
            "--json",
        ],
        None,
    );
    let json: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(json["totals"]["totalCost"], 1.0, "{output}");
}

#[test]
fn unified_sessions_count_the_in_window_usage_of_a_session_active_past_until() {
    let fixture = Fixture::new();
    fixture.write_file(
        "projects/project-a/session-e.jsonl",
        [
            usage_line(
                "2026-09-11T23:30:00.000Z",
                "session-e",
                "msg-in-window",
                "req-in-window",
                1.0,
            ),
            usage_line(
                "2026-09-12T00:30:00.000Z",
                "session-e",
                "msg-after-until",
                "req-after-until",
                2.0,
            ),
        ]
        .join("\n"),
    );
    let report = |command: &[&str]| {
        let mut args = command.to_vec();
        args.extend([
            "--since",
            "20260911",
            "--until",
            "20260911",
            "--timezone",
            "UTC",
            "--mode",
            "display",
            "--offline",
            "--json",
        ]);
        let output = run_ccusage(&fixture, &args, None);
        serde_json::from_str::<serde_json::Value>(&output).unwrap()["totals"]["totalCost"].clone()
    };

    assert_eq!(report(&["session"]), 1.0);
    assert_eq!(report(&["claude", "session"]), 1.0);
}

#[test]
fn claude_sessions_fold_workflow_runs_into_their_parent_session() {
    let fixture = Fixture::new();
    fixture.write_file(
        "projects/project-a/session-a.jsonl",
        usage_line(
            "2026-09-11T10:00:00.000Z",
            "session-a",
            "msg-main",
            "req-main",
            1.0,
        ),
    );
    fixture.write_file(
        "projects/project-a/session-a/subagents/agent-b.jsonl",
        sidechain_line(
            "2026-09-11T10:03:00.000Z",
            "session-a",
            "msg-subagent",
            "req-subagent",
            2.0,
        ),
    );
    fixture.write_file(
        "projects/project-a/session-a/subagents/workflows/wf_1/agent-c.jsonl",
        sidechain_line(
            "2026-09-11T10:05:00.000Z",
            "session-a",
            "msg-workflow",
            "req-workflow",
            4.0,
        ),
    );

    let output = run_ccusage(
        &fixture,
        &[
            "claude",
            "session",
            "--timezone",
            "UTC",
            "--mode",
            "display",
            "--offline",
            "--json",
        ],
        None,
    );
    let json: serde_json::Value = serde_json::from_str(&output).unwrap();
    let sessions = json["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1, "{output}");
    assert_eq!(sessions[0]["sessionId"], "session-a", "{output}");
    assert_eq!(sessions[0]["projectPath"], "project-a", "{output}");
    assert_eq!(sessions[0]["totalCost"], 7.0, "{output}");
}

#[test]
fn reads_transcripts_below_a_config_directory_named_under_projects() {
    let fixture = Fixture::new();
    let config_dir = fixture.root().join("projects/cfg");
    fixture.write_file(
        "projects/cfg/projects/myproj/session-1.jsonl",
        usage_line(
            "2026-09-11T10:00:00.000Z",
            "session-1",
            "msg-main",
            "req-main",
            1.0,
        ),
    );
    let command = || {
        let mut command = support::hermetic_command(fixture.root());
        command.env("CLAUDE_CONFIG_DIR", &config_dir);
        command
    };

    let session: serde_json::Value = serde_json::from_str(&run_command(
        command(),
        &[
            "claude",
            "session",
            "--timezone",
            "UTC",
            "--mode",
            "display",
            "--offline",
            "--json",
        ],
        None,
    ))
    .unwrap();
    let sessions = session["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1, "{session}");
    assert_eq!(sessions[0]["sessionId"], "session-1", "{session}");
    assert_eq!(sessions[0]["projectPath"], "myproj", "{session}");

    let daily: serde_json::Value = serde_json::from_str(&run_command(
        command(),
        &[
            "claude",
            "daily",
            "--timezone",
            "UTC",
            "--mode",
            "display",
            "--offline",
            "--json",
            "--instances",
            "--project",
            "myproj",
        ],
        None,
    ))
    .unwrap();
    let projects = daily["projects"].as_object().unwrap();
    assert!(projects.contains_key("myproj"), "{daily}");
    assert!(!projects.contains_key("cfg"), "{daily}");
}

#[test]
fn unified_sessions_keep_a_session_last_active_on_the_until_day() {
    let fixture = Fixture::new();
    fixture.write_file(
        "projects/project-a/session-c.jsonl",
        usage_line(
            "2026-09-11T23:30:00.000Z",
            "session-c",
            "msg-until",
            "req-until",
            1.0,
        ),
    );
    fixture.write_file(
        "projects/project-a/session-d.jsonl",
        usage_line(
            "2026-09-12T00:30:00.000Z",
            "session-d",
            "msg-after",
            "req-after",
            2.0,
        ),
    );

    let output = run_ccusage(
        &fixture,
        &[
            "session",
            "--until",
            "20260911",
            "--timezone",
            "UTC",
            "--mode",
            "display",
            "--offline",
            "--json",
        ],
        None,
    );
    let json: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(json["totals"]["totalCost"], 1.0, "{output}");
}

fn write_block_chain(
    fixture: &Fixture,
    last_entry: Instant,
    entries: u32,
) -> Vec<std::path::PathBuf> {
    let timestamps = (0..entries)
        .rev()
        .map(|back| last_entry.minus(HOUR * 4 * back))
        .collect::<Vec<_>>();
    let mut files = Vec::new();
    let mut chunks = vec![&timestamps[..1]];
    chunks.extend(timestamps[1..].chunks(2));
    for (index, chunk) in chunks.into_iter().enumerate() {
        let lines = chunk
            .iter()
            .map(|&timestamp| {
                let ms = timestamp.millis();
                usage_line(
                    &timestamp.rfc3339_millis(),
                    "session-chain",
                    &format!("msg-{ms}"),
                    &format!("req-{ms}"),
                    1.0,
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let file = fixture.write_file(format!("projects/chain/part-{index:03}.jsonl"), lines);
        set_modified(
            &file,
            *chunk.last().expect("the log batch has a last instant"),
        );
        files.push(file);
    }
    files
}

#[test]
fn bounded_blocks_keep_anchors_from_older_files() {
    let fixture = Fixture::new();
    let files = write_block_chain(&fixture, instant("2026-09-11T21:00:00Z"), 71);

    assert_output_ignores_mtimes(
        || claude_command(&fixture),
        &files,
        instant(NOW),
        &[
            "claude",
            "blocks",
            "--since",
            "20260911",
            "--timezone",
            "UTC",
            "--mode",
            "display",
            "--offline",
            "--json",
        ],
        None,
    );
}

const NOW: &str = "2026-09-12T12:00:00Z";
const HOUR: Duration = Duration::from_secs(60 * 60);

fn claude_command_at_now(fixture: &Fixture) -> std::process::Command {
    let mut command = claude_command(fixture);
    command.env("CCUSAGE_TEST_NOW", NOW);
    command
}

const STATUSLINE_ARGS: [&str; 7] = [
    "statusline",
    "--offline",
    "--no-cache",
    "--cost-source",
    "cc",
    "--timezone",
    "UTC",
];

fn statusline_hook(session: &str, transcript: &std::path::Path) -> String {
    serde_json::json!({
        "session_id": session,
        "transcript_path": transcript,
        "model": {"display_name": "Model"},
        "cost": {"total_cost_usd": 0},
        "context_window": {"total_input_tokens": 1, "context_window_size": 100},
    })
    .to_string()
}

#[test]
fn statusline_active_block_keeps_anchors_from_older_files() {
    let fixture = Fixture::new();
    let files = write_block_chain(&fixture, instant(NOW).minus(HOUR), 71);
    let hook = statusline_hook("session-chain", files.last().unwrap());

    let output = assert_output_ignores_mtimes(
        || claude_command_at_now(&fixture),
        &files,
        instant(NOW),
        &STATUSLINE_ARGS,
        Some(&hook),
    );
    assert_eq!(
        output,
        "🤖 Model | 💰 $0.00 session / $3.00 today / $1.00 block (4h 0m left) | 🧠 1 (1%)\n"
    );
}

#[test]
fn bounded_unified_reports_still_detect_claude_with_only_stale_files() {
    let fixture = Fixture::new();
    let old = fixture.write_file(
        "projects/project-a/old.jsonl",
        usage_line(
            "2026-09-01T12:00:00.000Z",
            "session-old",
            "msg-old",
            "req-old",
            1.0,
        ),
    );

    set_modified(&old, instant("2026-09-01T12:00:00Z"));

    let output = assert_output_ignores_mtimes(
        || claude_command(&fixture),
        &[old],
        instant(NOW),
        &[
            "daily",
            "--since",
            "20260910",
            "--timezone",
            "UTC",
            "--mode",
            "display",
            "--offline",
        ],
        None,
    );
    assert!(output.contains("Detected: Claude"), "{output}");
}

#[test]
fn statusline_keeps_the_current_block_before_future_dated_entries() {
    let fixture = Fixture::new();
    let now = instant(NOW);
    let transcript = fixture.write_file(
        "projects/project-a/session-now.jsonl",
        [
            usage_line(
                &now.minus(HOUR).rfc3339_millis(),
                "session-now",
                "msg-now",
                "req-now",
                1.0,
            ),
            usage_line(
                &now.plus(HOUR * 8).rfc3339_millis(),
                "session-now",
                "msg-future",
                "req-future",
                9.0,
            ),
        ]
        .join("\n"),
    );
    let hook = statusline_hook("session-now", &transcript);

    let output = run_command(
        claude_command_at_now(&fixture),
        &STATUSLINE_ARGS,
        Some(&hook),
    );
    assert_eq!(
        output,
        "🤖 Model | 💰 $0.00 session / $10.00 today / $1.00 block (4h 0m left) | 🧠 1 (1%)\n"
    );
}

#[test]
fn bounded_reports_dedupe_replays_filed_under_another_session_directory() {
    let fixture = Fixture::new();
    let parent = fixture.write_file(
        "projects/project-a/session-a.jsonl",
        usage_line(
            "2026-09-01T12:00:00.000Z",
            "session-a",
            "msg-parent",
            "req-parent",
            1.0,
        ),
    );
    let workflow = fixture.write_file(
        "projects/project-a/session-b/subagents/workflows/wf_1/agent-a.jsonl",
        [
            sidechain_line(
                "2026-09-11T12:00:00.000Z",
                "session-a",
                "msg-parent",
                "req-replay",
                2.0,
            ),
            sidechain_line(
                "2026-09-11T12:05:00.000Z",
                "session-a",
                "msg-answer",
                "req-answer",
                1.0,
            ),
        ]
        .join("\n"),
    );

    set_modified(&parent, instant("2026-09-01T12:00:00Z"));
    set_modified(&workflow, instant("2026-09-11T12:05:00Z"));

    let output = assert_output_ignores_mtimes(
        || claude_command(&fixture),
        &[parent, workflow],
        instant(NOW),
        &[
            "claude",
            "daily",
            "--since",
            "20260910",
            "--timezone",
            "UTC",
            "--mode",
            "display",
            "--offline",
            "--json",
        ],
        None,
    );
    let json: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(json["totals"]["totalCost"], 1.0, "{output}");
}

fn requestless_line(
    timestamp: &str,
    session: &str,
    message: &str,
    input_tokens: u64,
    cache_read_tokens: u64,
    is_sidechain: bool,
) -> String {
    format!(
        r#"{{"timestamp":"{timestamp}","sessionId":"{session}","isSidechain":{is_sidechain},"message":{{"id":"{message}","model":"claude-sonnet-4-20250514","usage":{{"input_tokens":{input_tokens},"output_tokens":2,"cache_read_input_tokens":{cache_read_tokens}}}}}}}"#
    )
}

fn claude_report_totals(fixture: &Fixture) -> Vec<(&'static str, u64)> {
    ["daily", "weekly", "monthly", "session", "blocks"]
        .into_iter()
        .map(|command| {
            let output = run_ccusage(
                fixture,
                &[
                    "claude",
                    command,
                    "--since",
                    "20260901",
                    "--until",
                    "20260930",
                    "--timezone",
                    "UTC",
                    "--mode",
                    "display",
                    "--offline",
                    "--json",
                ],
                None,
            );
            let json: serde_json::Value =
                serde_json::from_str(&output).expect("ccusage emitted JSON");
            let total = if command == "blocks" {
                json["blocks"]
                    .as_array()
                    .expect("blocks report lists blocks")
                    .iter()
                    .map(|block| {
                        block["totalTokens"]
                            .as_u64()
                            .expect("block has total tokens")
                    })
                    .sum()
            } else {
                json["totals"]["totalTokens"]
                    .as_u64()
                    .expect("report has total tokens")
            };
            (command, total)
        })
        .collect()
}

struct ChainedLine<'a> {
    uuid: &'a str,
    parent: Option<&'a str>,
    timestamp: &'a str,
    message: &'a str,
    input_tokens: u64,
    tool_use: Option<&'a str>,
}

fn chained_requestless_line(line: ChainedLine<'_>) -> String {
    let parent = line
        .parent
        .map_or_else(|| "null".to_string(), |parent| format!(r#""{parent}""#));
    let content = line.tool_use.map_or_else(
        || r#"[{"type":"text","text":"ok"}]"#.to_string(),
        |id| format!(r#"[{{"type":"tool_use","id":"{id}","name":"Read","input":{{}}}}]"#),
    );
    format!(
        r#"{{"parentUuid":{parent},"type":"assistant","uuid":"{uuid}","timestamp":"{timestamp}","sessionId":"session-a","message":{{"id":"{message}","model":"claude-sonnet-4-20250514","role":"assistant","content":{content},"usage":{{"input_tokens":{input_tokens},"output_tokens":2,"cache_read_input_tokens":0}}}}}}"#,
        uuid = line.uuid,
        timestamp = line.timestamp,
        message = line.message,
        input_tokens = line.input_tokens,
    )
}

fn tool_result_line(uuid: &str, parent: &str, tool_use_id: &str) -> String {
    format!(
        r#"{{"parentUuid":"{parent}","type":"user","uuid":"{uuid}","timestamp":"2026-09-11T12:00:00.250Z","sessionId":"session-a","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"{tool_use_id}","content":"ok"}}]}}}}"#
    )
}

fn prompt_line(uuid: &str, parent: &str) -> String {
    format!(
        r#"{{"parentUuid":"{parent}","type":"user","uuid":"{uuid}","timestamp":"2026-09-11T12:00:00.250Z","sessionId":"session-a","message":{{"role":"user","content":"next question"}}}}"#
    )
}

fn two_block_response(message: &str) -> [String; 2] {
    [
        chained_requestless_line(ChainedLine {
            uuid: "a1",
            parent: None,
            timestamp: "2026-09-11T12:00:00.000Z",
            message,
            input_tokens: 100,
            tool_use: None,
        }),
        chained_requestless_line(ChainedLine {
            uuid: "a2",
            parent: Some("a1"),
            timestamp: "2026-09-11T12:00:00.500Z",
            message,
            input_tokens: 100,
            tool_use: None,
        }),
    ]
}

#[test]
fn requestless_sidechain_replays_are_counted_once_in_every_report() {
    let fixture = Fixture::new();
    fixture.write_file(
        "projects/project-a/session-a.jsonl",
        requestless_line(
            "2026-09-11T12:00:00.000Z",
            "session-a",
            "msg-parent",
            10,
            20,
            false,
        ),
    );
    fixture.write_file(
        "projects/project-a/session-a/subagents/agent-a.jsonl",
        requestless_line(
            "2026-09-11T12:05:00.000Z",
            "session-a",
            "msg-parent",
            10,
            50_000,
            true,
        ),
    );

    for (command, total) in claude_report_totals(&fixture) {
        assert_eq!(total, 32, "claude {command}");
    }
}

#[test]
fn gateway_responses_reusing_one_message_id_are_counted_in_every_report() {
    let fixture = Fixture::new();
    let lines = Vec::from_fn(3, |minute| {
        requestless_line(
            &format!("2026-09-1{}T12:0{minute}:00.000Z", minute + 1),
            "session-a",
            "ocgo",
            100,
            0,
            false,
        )
    });
    fixture.write_file("projects/project-a/session-a.jsonl", lines.join("\n"));

    for (command, total) in claude_report_totals(&fixture) {
        assert_eq!(total, 306, "claude {command}");
    }
}

#[test]
fn requestless_anthropic_message_id_content_blocks_are_counted_once_in_every_report() {
    let fixture = Fixture::new();
    fixture.write_file(
        "projects/project-a/session-a.jsonl",
        two_block_response("msg_01AbC2DeF3").join("\n"),
    );

    for (command, total) in claude_report_totals(&fixture) {
        assert_eq!(total, 102, "claude {command}");
    }
}

#[test]
fn requestless_response_message_id_content_blocks_are_counted_once_in_every_report() {
    let fixture = Fixture::new();
    fixture.write_file(
        "projects/project-a/session-a.jsonl",
        two_block_response("resp_0123456789abcdef").join("\n"),
    );

    for (command, total) in claude_report_totals(&fixture) {
        assert_eq!(total, 102, "claude {command}");
    }
}

#[test]
fn requestless_proxy_message_id_content_blocks_are_counted_once_in_every_report() {
    let fixture = Fixture::new();
    fixture.write_file(
        "projects/project-a/session-a.jsonl",
        two_block_response("gateway-7f3a").join("\n"),
    );

    for (command, total) in claude_report_totals(&fixture) {
        assert_eq!(total, 102, "claude {command}");
    }
}

#[test]
fn requestless_content_blocks_around_their_own_tool_results_are_counted_once_in_every_report() {
    let fixture = Fixture::new();
    let lines = [
        chained_requestless_line(ChainedLine {
            uuid: "a1",
            parent: None,
            timestamp: "2026-09-11T12:00:00.000Z",
            message: "gateway-7f3a",
            input_tokens: 100,
            tool_use: Some("toolu_1"),
        }),
        tool_result_line("u1", "a1", "toolu_1"),
        chained_requestless_line(ChainedLine {
            uuid: "a2",
            parent: Some("u1"),
            timestamp: "2026-09-11T12:00:00.500Z",
            message: "gateway-7f3a",
            input_tokens: 100,
            tool_use: Some("toolu_2"),
        }),
    ];
    fixture.write_file("projects/project-a/session-a.jsonl", lines.join("\n"));

    for (command, total) in claude_report_totals(&fixture) {
        assert_eq!(total, 102, "claude {command}");
    }
}

#[test]
fn requestless_message_id_reused_after_a_tool_result_with_new_input_is_counted_twice_in_every_report()
 {
    let fixture = Fixture::new();
    let lines = [
        chained_requestless_line(ChainedLine {
            uuid: "a1",
            parent: None,
            timestamp: "2026-09-11T12:00:00.000Z",
            message: "msg_01AbC2DeF3",
            input_tokens: 100,
            tool_use: Some("toolu_1"),
        }),
        tool_result_line("u1", "a1", "toolu_1"),
        chained_requestless_line(ChainedLine {
            uuid: "a2",
            parent: Some("u1"),
            timestamp: "2026-09-11T12:00:01.000Z",
            message: "msg_01AbC2DeF3",
            input_tokens: 150,
            tool_use: None,
        }),
    ];
    fixture.write_file("projects/project-a/session-a.jsonl", lines.join("\n"));

    for (command, total) in claude_report_totals(&fixture) {
        assert_eq!(total, 254, "claude {command}");
    }
}

#[test]
fn requestless_content_blocks_whose_tool_input_holds_a_uuid_are_counted_once_in_every_report() {
    let fixture = Fixture::new();
    let [first, second] = two_block_response("msg_01AbC2DeF3");
    let first = first
        .replace(
            r#""content":[{"type":"text","text":"ok"}]"#,
            r#""content":[{"type":"tool_use","id":"toolu_1","name":"Read","input":{"path":"a","uuid":"x"}}]"#,
        );
    fixture.write_file(
        "projects/project-a/session-a.jsonl",
        [first, second].join("\n"),
    );

    for (command, total) in claude_report_totals(&fixture) {
        assert_eq!(total, 102, "claude {command}");
    }
}

#[test]
fn requestless_message_id_reused_after_a_user_prompt_is_counted_twice_in_every_report() {
    let fixture = Fixture::new();
    let lines = [
        chained_requestless_line(ChainedLine {
            uuid: "a1",
            parent: None,
            timestamp: "2026-09-11T12:00:00.000Z",
            message: "msg_01AbC2DeF3",
            input_tokens: 100,
            tool_use: None,
        }),
        prompt_line("u1", "a1"),
        chained_requestless_line(ChainedLine {
            uuid: "a2",
            parent: Some("u1"),
            timestamp: "2026-09-11T12:00:01.000Z",
            message: "msg_01AbC2DeF3",
            input_tokens: 100,
            tool_use: None,
        }),
    ];
    fixture.write_file("projects/project-a/session-a.jsonl", lines.join("\n"));

    for (command, total) in claude_report_totals(&fixture) {
        assert_eq!(total, 204, "claude {command}");
    }
}

#[test]
fn requestless_anthropic_message_id_keeps_largest_usage_across_days_and_sessions() {
    let fixture = Fixture::new();
    let line = |uuid: &str, parent: &str, timestamp: &str, session: &str, output_tokens: u64| {
        format!(
            r#"{{"uuid":"{uuid}","parentUuid":{parent},"timestamp":"{timestamp}","sessionId":"{session}","message":{{"id":"msg_01AbC2DeF3","model":"claude-sonnet-4-20250514","usage":{{"input_tokens":100,"output_tokens":{output_tokens}}}}}}}"#
        )
    };
    let session_a = [
        line("a1", "null", "2026-09-11T23:59:59.900Z", "session-a", 5),
        line("a2", r#""a1""#, "2026-09-12T00:00:00.100Z", "session-a", 50),
    ];
    fixture.write_file("projects/project-a/session-a.jsonl", session_a.join("\n"));
    fixture.write_file(
        "projects/project-a/session-b.jsonl",
        line("b1", "null", "2026-09-12T01:00:00.000Z", "session-b", 5),
    );

    for (command, total) in claude_report_totals(&fixture) {
        assert_eq!(total, 150 + 105, "claude {command}");
    }
}

#[test]
fn auto_mode_breakdowns_report_the_stored_part_of_cost() {
    let fixture = Fixture::new();
    let lines = [
        r#"{"timestamp":"2026-09-15T12:00:00.000Z","sessionId":"session-a","requestId":"request-a","message":{"id":"message-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":1000,"output_tokens":100}}}"#,
        r#"{"timestamp":"2026-09-15T12:01:00.000Z","sessionId":"session-a","requestId":"request-b","costUSD":0.5,"message":{"id":"message-b","model":"claude-sonnet-4-20250514","usage":{"input_tokens":500,"output_tokens":10}}}"#,
    ];
    fixture.write_file("projects/project-a/session-a/chat.jsonl", lines.join("\n"));

    let breakdown = |args: &[&str], pointer: &str| -> serde_json::Value {
        let base = ["--json", "--offline", "--timezone", "UTC"];
        let args = args.iter().chain(&base).copied().collect::<Vec<_>>();
        let json: serde_json::Value =
            serde_json::from_str(&run_ccusage(&fixture, &args, None)).unwrap();
        json.pointer(pointer).unwrap().clone()
    };
    let reports = [
        (&["claude", "daily"][..], "/daily/0/modelBreakdowns/0"),
        (&["claude", "session"][..], "/sessions/0/modelBreakdowns/0"),
        (
            &["claude", "session", "--id", "session-a"][..],
            "/modelBreakdowns/0",
        ),
        (&["claude", "monthly"][..], "/monthly/0/modelBreakdowns/0"),
        (
            &["daily", "--by-agent"][..],
            "/daily/0/agents/0/modelBreakdowns/0",
        ),
    ];

    for (args, pointer) in reports {
        let auto = breakdown(args, pointer);
        assert_eq!(auto["storedCost"], 0.5, "{args:?}: {auto}");
        assert!(auto["cost"].as_f64().unwrap() > 0.5, "{args:?}: {auto}");

        for mode in ["calculate", "display"] {
            let args = args
                .iter()
                .copied()
                .chain(["--mode", mode])
                .collect::<Vec<_>>();
            let other = breakdown(&args, pointer);
            assert!(other.get("storedCost").is_none(), "{args:?}: {other}");
        }

        let args = args
            .iter()
            .copied()
            .chain(["--no-cost"])
            .collect::<Vec<_>>();
        let stripped = breakdown(&args, pointer);
        assert!(stripped.get("storedCost").is_none(), "{args:?}: {stripped}");
    }
}

#[test]
fn breakdowns_report_one_hour_cache_creation_tokens_only_for_models_with_a_split() {
    let fixture = Fixture::new();
    let lines = [
        r#"{"timestamp":"2026-09-15T12:00:00.000Z","sessionId":"session-a","requestId":"request-a","message":{"id":"message-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":2,"cache_creation_input_tokens":400,"cache_creation":{"ephemeral_5m_input_tokens":100,"ephemeral_1h_input_tokens":300}}}}"#,
        r#"{"timestamp":"2026-09-15T12:01:00.000Z","sessionId":"session-a","requestId":"request-b","message":{"id":"message-b","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":2,"cache_creation_input_tokens":50}}}"#,
        r#"{"timestamp":"2026-09-15T12:02:00.000Z","sessionId":"session-a","requestId":"request-c","message":{"id":"message-c","model":"claude-haiku-4-5-20251001","usage":{"input_tokens":10,"output_tokens":2,"cache_creation_input_tokens":70}}}"#,
        r#"{"timestamp":"2026-09-16T12:00:00.000Z","sessionId":"session-a","requestId":"request-d","message":{"id":"message-d","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":2,"cache_creation_input_tokens":100,"cache_creation":{"ephemeral_5m_input_tokens":20,"ephemeral_1h_input_tokens":80}}}}"#,
        r#"{"timestamp":"2026-09-16T12:01:00.000Z","sessionId":"session-a","requestId":"request-e","message":{"id":"message-e","model":"claude-haiku-4-5-20251001","usage":{"input_tokens":10,"output_tokens":2,"cache_creation_input_tokens":30}}}"#,
    ];
    fixture.write_file("projects/project-a/session-a/chat.jsonl", lines.join("\n"));

    let first_day = (450, 300, 70);
    let both_days = (550, 380, 100);
    let reports = [
        (
            &["claude", "daily"][..],
            "/daily/0/modelBreakdowns",
            first_day,
        ),
        (
            &["claude", "weekly"][..],
            "/weekly/0/modelBreakdowns",
            both_days,
        ),
        (
            &["claude", "monthly"][..],
            "/monthly/0/modelBreakdowns",
            both_days,
        ),
        (
            &["claude", "session"][..],
            "/sessions/0/modelBreakdowns",
            both_days,
        ),
        (&["daily"][..], "/daily/0/modelBreakdowns", first_day),
        (&["weekly"][..], "/weekly/0/modelBreakdowns", both_days),
        (&["monthly"][..], "/monthly/0/modelBreakdowns", both_days),
        (&["session"][..], "/session/0/modelBreakdowns", both_days),
        (
            &["daily", "--by-agent"][..],
            "/daily/0/agents/0/modelBreakdowns",
            first_day,
        ),
    ];
    for (args, pointer, (sonnet_cache, sonnet_1h, haiku_cache)) in reports {
        let until: &[&str] = if pointer.starts_with("/daily") {
            &["--until", "2026-09-15"]
        } else {
            &[]
        };
        let args = args
            .iter()
            .chain(&["--json", "--offline", "--timezone", "UTC"])
            .chain(until)
            .copied()
            .collect::<Vec<_>>();
        let json: serde_json::Value =
            serde_json::from_str(&run_ccusage(&fixture, &args, None)).unwrap();
        let breakdowns = json.pointer(pointer).unwrap().as_array().unwrap();
        let model = |name: &str| {
            breakdowns
                .iter()
                .find(|b| b["modelName"] == name)
                .unwrap_or_else(|| panic!("{args:?}: no {name} in {breakdowns:?}"))
        };

        let sonnet = model("claude-sonnet-4-20250514");
        assert_eq!(
            sonnet["cacheCreationTokens"], sonnet_cache,
            "{args:?}: {sonnet}"
        );
        assert_eq!(
            sonnet["cacheCreation1hTokens"], sonnet_1h,
            "{args:?}: {sonnet}"
        );

        let haiku = model("claude-haiku-4-5-20251001");
        assert_eq!(
            haiku["cacheCreationTokens"], haiku_cache,
            "{args:?}: {haiku}"
        );
        assert!(
            haiku.get("cacheCreation1hTokens").is_none(),
            "{args:?}: {haiku}"
        );
    }
}

#[test]
fn breakdowns_keep_the_one_hour_split_from_an_otherwise_equal_duplicate() {
    let unsplit = r#"{"timestamp":"2026-09-15T12:00:00.000Z","sessionId":"session-a","requestId":"request-a","message":{"id":"message-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":2,"cache_creation_input_tokens":400}}}"#;
    let split = r#"{"timestamp":"2026-09-15T12:00:00.000Z","sessionId":"session-a","requestId":"request-a","message":{"id":"message-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":2,"cache_creation_input_tokens":400,"cache_creation":{"ephemeral_5m_input_tokens":100,"ephemeral_1h_input_tokens":300}}}}"#;

    for lines in [[unsplit, split], [split, unsplit]] {
        let fixture = Fixture::new();
        fixture.write_file("projects/project-a/session-a/chat.jsonl", lines.join("\n"));

        let mut costs = Vec::new();
        for (report, pointer) in [("daily", "/daily/0"), ("session", "/sessions/0")] {
            let args = [
                "claude",
                report,
                "--json",
                "--offline",
                "--mode",
                "calculate",
                "--timezone",
                "UTC",
            ];
            let json: serde_json::Value =
                serde_json::from_str(&run_ccusage(&fixture, &args, None)).unwrap();
            let row = json.pointer(pointer).unwrap();
            let breakdown = &row["modelBreakdowns"][0];

            assert_eq!(breakdown["cacheCreationTokens"], 400, "{args:?}: {row}");
            assert_eq!(breakdown["cacheCreation1hTokens"], 300, "{args:?}: {row}");
            costs.push(row["totalCost"].as_f64().unwrap());
        }
        assert_eq!(costs[0], costs[1], "daily and session cost: {costs:?}");
    }
}

#[test]
fn advisor_turns_count_the_cache_writes_of_every_message_iteration_in_every_report() {
    let fixture = Fixture::new();
    let line = r#"{"isSidechain":false,"message":{"model":"claude-sonnet-4-20250514","id":"msg_advisor","type":"message","role":"assistant","content":[{"type":"text","text":"placeholder"}],"stop_reason":"tool_use","stop_sequence":null,"usage":{"input_tokens":4,"cache_creation_input_tokens":12323,"cache_read_input_tokens":140030,"output_tokens":244,"service_tier":"standard","cache_creation":{"ephemeral_1h_input_tokens":11142,"ephemeral_5m_input_tokens":0},"iterations":[{"input_tokens":2,"output_tokens":58,"cache_read_input_tokens":64444,"cache_creation_input_tokens":11142,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":11142},"type":"message"},{"input_tokens":77025,"output_tokens":3427,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":0},"type":"advisor_message","model":"claude-opus-5"},{"input_tokens":2,"output_tokens":186,"cache_read_input_tokens":75586,"cache_creation_input_tokens":1181,"cache_creation":{"ephemeral_5m_input_tokens":1181,"ephemeral_1h_input_tokens":0},"type":"message"}],"speed":"standard"}},"requestId":"req_advisor","type":"assistant","uuid":"00000000-0000-4000-8000-000000000002","timestamp":"2026-09-15T12:00:00.000Z","cwd":"/tmp/demo","sessionId":"session-a","version":"2.1.323"}"#;
    fixture.write_file("projects/project-a/session-a.jsonl", line);

    let mut costs = Vec::new();
    for (report, pointer) in [
        ("daily", "/daily/0"),
        ("weekly", "/weekly/0"),
        ("monthly", "/monthly/0"),
        ("session", "/sessions/0"),
    ] {
        let args = [
            "claude",
            report,
            "--json",
            "--offline",
            "--mode",
            "calculate",
            "--timezone",
            "UTC",
        ];
        let json: serde_json::Value =
            serde_json::from_str(&run_ccusage(&fixture, &args, None)).unwrap();
        let row = json.pointer(pointer).unwrap();
        let sonnet = row["modelBreakdowns"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["modelName"] == "claude-sonnet-4-20250514")
            .unwrap_or_else(|| panic!("{args:?}: no main model in {row}"));

        assert_eq!(sonnet["cacheCreationTokens"], 12323, "{args:?}: {row}");
        assert_eq!(sonnet["cacheCreation1hTokens"], 11142, "{args:?}: {row}");
        assert_eq!(row["cacheCreationTokens"], 12323, "{args:?}: {row}");
        costs.push(row["totalCost"].as_f64().unwrap());
    }

    let blocks: serde_json::Value = serde_json::from_str(&run_ccusage(
        &fixture,
        &[
            "claude",
            "blocks",
            "--json",
            "--offline",
            "--mode",
            "calculate",
            "--timezone",
            "UTC",
        ],
        None,
    ))
    .unwrap();
    let block = blocks.pointer("/blocks/0").unwrap();
    assert_eq!(
        block["tokenCounts"]["cacheCreationInputTokens"], 12323,
        "{block}"
    );
    costs.push(block["costUSD"].as_f64().unwrap());

    assert!(
        costs
            .windows(2)
            .all(|pair| (pair[0] - pair[1]).abs() < 1e-9),
        "report costs differ: {costs:?}"
    );
}

fn run_with_user_config(fixture: &Fixture, claude_config_dir: Option<&str>) -> serde_json::Value {
    let mut command = support::hermetic_command(fixture.root());
    command.args(["claude", "daily", "--offline", "--mode", "display"]);
    if let Some(claude_config_dir) = claude_config_dir {
        command.env("CLAUDE_CONFIG_DIR", claude_config_dir);
    }
    let output = command.output().expect("failed to run ccusage");
    assert!(
        output.status.success(),
        "ccusage claude daily failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout)
        .expect("the user ccusage.json should switch the report to JSON")
}

const JSON_USER_CONFIG: &str = r#"{ "defaults": { "json": true } }"#;

#[test]
fn block_table_and_active_detail_follow_the_report_timezone() {
    let fixture = Fixture::new();
    fixture.write_file(
        "projects/p/session-a.jsonl",
        usage_line(
            "2026-01-02T23:00:00Z",
            "session-a",
            "message-a",
            "request-a",
            1.0,
        ),
    );
    for (flags, expected) in [
        (vec![], "1/3/2026, 8:00:00 AM"),
        (vec!["--active"], "2026-01-03 08:00:00"),
    ] {
        let output = claude_command(&fixture)
            .env("COLUMNS", "200")
            .env("CCUSAGE_TEST_NOW", "2026-01-03T00:00:00Z")
            .args(["claude", "blocks", "--offline", "--timezone", "Asia/Tokyo"])
            .args(flags)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(expected),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}

#[test]
fn colored_blocks_right_align_assumptions_like_plain_blocks() {
    let fixture = Fixture::new();
    fixture.write_file(
        "projects/p/session-a.jsonl",
        [
            usage_line(
                "2026-01-02T23:00:00Z",
                "session-a",
                "message-a",
                "request-a",
                1.0,
            ),
            usage_line(
                "2026-01-02T23:30:00Z",
                "session-a",
                "message-b",
                "request-b",
                1.0,
            ),
        ]
        .join("\n"),
    );
    let assumptions = |flag| {
        let output = claude_command(&fixture)
            .env_remove("NO_COLOR")
            .env("COLUMNS", "200")
            .env("CCUSAGE_TEST_NOW", "2026-01-03T00:00:00Z")
            .args([
                "claude",
                "blocks",
                "--offline",
                "--timezone",
                "UTC",
                "--token-limit",
                "100",
                flag,
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output.stderr);
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .filter(|line| line.contains("(assuming "))
            .map(|line| line.split('│').nth(1).unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    let plain = assumptions("--no-color");
    let colored = assumptions("--color");
    assert_eq!(plain.len(), 2);
    assert!(plain[0].starts_with("  "));
    assert!(colored.iter().all(|cell| cell.contains("\x1b[90m")));
    assert_eq!(
        colored
            .iter()
            .map(|cell| cell.replace("\x1b[90m", "").replace("\x1b[0m", ""))
            .collect::<Vec<_>>(),
        plain
    );
}

#[test]
fn blocks_reject_unrepresentable_session_lengths() {
    let fixture = Fixture::new();
    write_two_model_session(&fixture);
    for value in ["NaN", "inf", "1e300"] {
        let output = claude_command(&fixture)
            .args([
                "claude",
                "blocks",
                "--offline",
                "--json",
                "--session-length",
                value,
            ])
            .output()
            .unwrap();
        assert!(!output.status.success(), "accepted {value}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("Session length"));
    }
}

#[test]
fn config_warnings_follow_log_level() {
    let fixture = Fixture::new();
    write_two_model_session(&fixture);
    fixture.write_file("ccusage.json", r#"{"defaults":{"json":true,"jsoon":true}}"#);
    for (level, expected) in [("0", false), ("1", true)] {
        let output = claude_command(&fixture)
            .env("LOG_LEVEL", level)
            .args(["claude", "daily", "--offline"])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr).contains("defaults.jsoon"),
            expected
        );
        assert!(serde_json::from_slice::<serde_json::Value>(&output.stdout).is_ok());
    }
}

#[test]
fn configured_project_filters_focused_and_unified_claude_reports() {
    let fixture = project_filter_fixture();
    for report in ["daily", "weekly", "monthly", "session"] {
        for config in project_filter_configs(report) {
            for command in [&[report][..], &["claude", report]] {
                let json = report_with_project_config(&fixture, command, &config);
                assert_eq!(json["totals"]["totalCost"], 1.25, "{command:?}: {config}");
            }
        }
    }
}

#[test]
fn configured_project_filters_claude_blocks() {
    let fixture = project_filter_fixture();
    for config in project_filter_configs("blocks") {
        for command in [&["blocks"][..], &["claude", "blocks"]] {
            let json = report_with_project_config(&fixture, command, &config);
            let costs = json["blocks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|block| block["costUSD"].as_f64().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(costs, [1.25], "{command:?}: {config}");
        }
    }
}

#[test]
fn legacy_blocks_use_claude_config_for_project_and_token_limit() {
    let fixture = Fixture::new();
    for (project, cost) in [("a", 1.25), ("b", 2.5)] {
        fixture.write_file(
            format!("projects/{project}/session.jsonl"),
            [
                usage_line("2026-09-15T12:00:00Z", project, project, "first", cost),
                usage_line("2026-09-15T12:30:00Z", project, project, "second", 0.0),
            ]
            .join("\n"),
        );
    }
    let config = serde_json::from_str(
        r#"{"claude":{"commands":{"blocks":{"project":"a","tokenLimit":1000}}}}"#,
    )
    .unwrap();
    let json = report_with_project_config(&fixture, &["blocks"], &config);
    let blocks = json["blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0]["costUSD"], 1.25);
    assert_eq!(blocks[0]["tokenLimitStatus"]["limit"], 1000);
}

#[test]
fn explicit_claude_session_id_ignores_configured_project() {
    let fixture = project_filter_fixture();
    fixture.write_file(
        "projects/project-b/target.jsonl",
        usage_line("2026-09-15T12:00:00Z", "target", "target", "target", 4.0),
    );
    let config = serde_json::json!({"claude": {"defaults": {"project": "project-a"}}});
    for command in [
        &["session", "--id=target"][..],
        &["claude", "session", "-i", "target"],
    ] {
        let json = report_with_project_config(&fixture, command, &config);
        assert_eq!(json["totalCost"], 4.0, "{command:?}");
    }
}

#[test]
fn legacy_session_id_uses_claude_report_config() {
    let fixture = project_filter_fixture();
    let config = serde_json::json!({"claude": {"commands": {"session": {"since": "20260916"}}}});
    let json = report_with_project_config(&fixture, &["session", "-i", "session"], &config);
    assert!(json.is_null(), "{json}");
}

#[test]
fn unified_project_filter_preserves_other_agents_and_explains_its_scope() {
    let fixture = project_filter_fixture();
    fixture.write_file(
        "grok/sessions/other-project/grok-session/updates.jsonl",
        r#"{"timestamp":1789473600,"params":{"sessionId":"grok-session","update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":20,"outputTokens":10,"modelUsage":{"grok-4.5-build":{"inputTokens":20,"outputTokens":10}}}},"_meta":{"eventId":"grok-event"}}}"#,
    );
    let path = fixture.write_file("config.json", r#"{"defaults":{"project":"project-a"}}"#);
    let note = "The project filter applies only to Claude Code usage.";
    for report in ["daily", "weekly", "monthly", "session"] {
        let mut cmd = claude_command(&fixture);
        cmd.env("GROK_HOME", fixture.path("grok"));
        let output = run_command(
            cmd,
            &[
                report,
                "--offline",
                "--json",
                "--config",
                path.to_str().unwrap(),
            ],
            None,
        );
        let json: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(json["totals"]["totalTokens"], 42, "{report}: {json}");
        assert_eq!(json["notes"], serde_json::json!([note]), "{report}: {json}");
    }

    let args = [
        "weekly",
        "--sections",
        "monthly,session",
        "--offline",
        "--config",
        path.to_str().unwrap(),
    ];
    let table = run_ccusage_output(&fixture, &args, None);
    assert_eq!(
        String::from_utf8_lossy(&table.stderr).matches(note).count(),
        1
    );
    let json = run_ccusage(&fixture, &[&args[..], &["--json"]].concat(), None);
    let json: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(json["notes"], serde_json::json!([note]));
}

#[test]
fn daily_project_flag_overrides_configured_project() {
    let fixture = project_filter_fixture();
    let config = serde_json::json!({"claude": {"commands": {"daily": {"project": "project-a"}}}});
    let json = report_with_project_config(
        &fixture,
        &["claude", "daily", "--project", "project-b"],
        &config,
    );
    assert_eq!(json["totals"]["totalCost"], 2.5);
}

fn project_filter_fixture() -> Fixture {
    let fixture = Fixture::new();
    for (suffix, cost) in [("a", 1.25), ("b", 2.5)] {
        fixture.write_file(
            format!("projects/project-{suffix}/session.jsonl"),
            usage_line(
                "2026-09-15T12:00:00Z",
                "session",
                &format!("message-{suffix}"),
                &format!("request-{suffix}"),
                cost,
            ),
        );
    }
    fixture
}

fn project_filter_configs(report: &str) -> [serde_json::Value; 6] {
    use serde_json::json;

    [
        json!({"defaults": {"project": "project-a"}}),
        json!({"claude": {"defaults": {"project": "project-a"}}}),
        json!({"commands": {report: {"project": "project-a"}}}),
        json!({
            "defaults": {"project": "project-b"},
            "commands": {report: {"project": "project-a"}}
        }),
        json!({"commands": {
            (format!("claude {report}")): {"project": "project-b"},
            (format!("claude:{report}")): {"project": "project-a"}
        }}),
        json!({"claude": {
            "defaults": {"project": "project-b"},
            "commands": {report: {"project": "project-a"}}
        }}),
    ]
}

fn report_with_project_config(
    fixture: &Fixture,
    command: &[&str],
    config: &serde_json::Value,
) -> serde_json::Value {
    let path = fixture.write_file("config.json", config.to_string());
    let args = [
        command,
        &[
            "--offline",
            "--json",
            "--mode",
            "display",
            "--config",
            path.to_str().unwrap(),
        ],
    ]
    .concat();
    let mut cmd = claude_command(fixture);
    cmd.env("CCUSAGE_TEST_NOW", "2026-09-15T13:00:00Z");
    serde_json::from_str(&run_command(cmd, &args, None)).unwrap()
}

#[test]
fn user_config_beside_the_logs_in_xdg_config_home_is_loaded() {
    let fixture = Fixture::new();
    fixture.write_file(
        "xdg-config/claude/projects/project-a/session-a.jsonl",
        usage_line(
            "2026-09-15T12:00:00.000Z",
            "session-a",
            "message-a",
            "request-a",
            1.0,
        ),
    );
    fixture.write_file("xdg-config/claude/ccusage.json", JSON_USER_CONFIG);

    let json = run_with_user_config(&fixture, None);

    assert_eq!(json["totals"]["totalCost"], 1.0);
}

#[test]
fn user_config_in_a_tilde_claude_config_dir_entry_is_loaded() {
    let fixture = Fixture::new();
    fixture.write_file(
        "home/claude-a/projects/project-a/session-a.jsonl",
        usage_line(
            "2026-09-15T12:00:00.000Z",
            "session-a",
            "message-a",
            "request-a",
            1.0,
        ),
    );
    fixture.write_file("home/claude-b/ccusage.json", JSON_USER_CONFIG);

    let json = run_with_user_config(&fixture, Some("~/claude-a,~/claude-b"));

    assert_eq!(json["totals"]["totalCost"], 1.0);
}
