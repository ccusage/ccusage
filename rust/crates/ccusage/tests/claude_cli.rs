use ccusage_test_support::Fixture;

#[test]
fn session_id_deduplicates_repeated_message_usage() {
    let fixture = Fixture::new();
    let messages = [
        r#"{"timestamp":"2026-09-15T12:00:00.000Z","sessionId":"session-a","requestId":"request-a","costUSD":1.25,"message":{"id":"message-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":2}}}"#,
        r#"{"timestamp":"2026-09-15T12:00:01.000Z","sessionId":"session-a","requestId":"request-a","costUSD":1.25,"message":{"id":"message-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":2}}}"#,
        r#"{"timestamp":"2026-09-15T12:00:02.000Z","sessionId":"session-a","requestId":"request-a","costUSD":1.25,"message":{"id":"message-a","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":2}}}"#,
    ];
    let _ = fixture.write_file(
        "projects/project-a/session-a/chat.jsonl",
        messages.join("\n"),
    );

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ccusage"))
        .env_clear()
        .env("HOME", fixture.path("home"))
        .env("USERPROFILE", fixture.path("userprofile"))
        .env("XDG_CONFIG_HOME", fixture.path("xdg-config"))
        .env("CLAUDE_CONFIG_DIR", fixture.root())
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

fn set_file_modified(path: &std::path::Path, unix_seconds: u64) {
    let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(unix_seconds);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
}

fn run_bounded_daily(fixture: &Fixture, extra_args: &[&str]) -> std::process::Output {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ccusage"))
        .env_clear()
        .env("HOME", fixture.path("home"))
        .env("USERPROFILE", fixture.path("userprofile"))
        .env("XDG_CONFIG_HOME", fixture.path("xdg-config"))
        .env("CLAUDE_CONFIG_DIR", fixture.root())
        .env("NO_COLOR", "1")
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

    // Written long before --since: holds only pre-window history.
    let stale = fixture.write_file(
        "projects/project-a/stale.jsonl",
        entry("2026-09-01T12:00:00.000Z", "stale", 4.0),
    );
    // Its mtime is inside the 24 hour margin, so it is still read even though
    // its entry was flushed after it happened.
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
    // 2026-09-01T12:00:00Z, 2026-09-09T06:00:00Z, 2026-09-11T12:00:00Z
    set_file_modified(&stale, 1_788_264_000);
    set_file_modified(&margin, 1_788_933_600);
    set_file_modified(&fresh, 1_789_128_000);

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

// 2026-09-12T12:00:00Z, later than every fixture entry below.
const FRESH_MTIME: u64 = 1_789_214_400;

fn run_ccusage(fixture: &Fixture, args: &[&str], stdin: Option<&str>) -> String {
    use std::io::Write as _;

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_ccusage"))
        .env_clear()
        .env("HOME", fixture.path("home"))
        .env("USERPROFILE", fixture.path("userprofile"))
        .env("XDG_CONFIG_HOME", fixture.path("xdg-config"))
        .env("CLAUDE_CONFIG_DIR", fixture.root())
        .env("NO_COLOR", "1")
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to run ccusage");
    let mut child_stdin = child.stdin.take().unwrap();
    child_stdin
        .write_all(stdin.unwrap_or_default().as_bytes())
        .unwrap();
    drop(child_stdin);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "ccusage {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// Runs `args` twice: once with the fixture's own mtimes and once with every
/// file touched to a fresh mtime. mtime pruning must never change a report.
fn assert_output_ignores_mtimes(
    fixture: &Fixture,
    files: &[std::path::PathBuf],
    fresh_mtime: u64,
    args: &[&str],
    stdin: Option<&str>,
    normalize: impl Fn(&str) -> String,
) -> String {
    let stale = normalize(&run_ccusage(fixture, args, stdin));
    for file in files {
        set_file_modified(file, fresh_mtime);
    }
    let fresh = normalize(&run_ccusage(fixture, args, stdin));
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
        // 2026-09-01T12:00:00Z and 2026-09-11T12:05:00Z
        set_file_modified(&parent, 1_788_264_000);
        set_file_modified(&sidechain, 1_789_128_300);
        let output = assert_output_ignores_mtimes(
            &fixture,
            &files,
            FRESH_MTIME,
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
            str::to_string,
        );
        if command == "daily" {
            let json: serde_json::Value = serde_json::from_str(&output).unwrap();
            assert_eq!(json["totals"]["totalCost"], 1.0, "{output}");
        }
    }
}

#[test]
fn bounded_unified_sessions_keep_old_subagent_files() {
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
    // 2026-09-11T12:00:00Z and 2026-09-01T12:00:00Z
    set_file_modified(&parent, 1_789_128_000);
    set_file_modified(&subagent, 1_788_264_000);

    let output = assert_output_ignores_mtimes(
        &fixture,
        &[parent, subagent],
        FRESH_MTIME,
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
        str::to_string,
    );
    let json: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(json["totals"]["totalCost"], 3.0, "{output}");
}

/// Writes a continuous chain of usage entries 4 hours apart, never idle long
/// enough to reset a 5 hour block. Files pair entries so they straddle block
/// boundaries: dropping whole early files re-anchors every later block.
fn write_block_chain(
    fixture: &Fixture,
    last_entry_ms: i64,
    entries: i64,
) -> Vec<std::path::PathBuf> {
    const STEP_MS: i64 = 4 * 60 * 60 * 1000;
    let timestamps = (0..entries)
        .rev()
        .map(|back| last_entry_ms - back * STEP_MS)
        .collect::<Vec<_>>();
    let mut files = Vec::new();
    // The earliest entry opens block 0 alone in its file; later files each hold
    // the second entry of one block and the first entry of the next.
    let mut chunks = vec![&timestamps[..1]];
    chunks.extend(timestamps[1..].chunks(2));
    for (index, chunk) in chunks.into_iter().enumerate() {
        let lines = chunk
            .iter()
            .map(|&ms| {
                let timestamp =
                    ccusage_core::format_rfc3339_millis(ccusage_core::TimestampMs::from_millis(ms));
                usage_line(
                    &timestamp,
                    "session-chain",
                    &format!("msg-{ms}"),
                    &format!("req-{ms}"),
                    1.0,
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let file = fixture.write_file(format!("projects/chain/part-{index:03}.jsonl"), lines);
        set_file_modified(&file, u64::try_from(chunk.last().unwrap() / 1000).unwrap());
        files.push(file);
    }
    files
}

#[test]
fn bounded_blocks_keep_anchors_from_older_files() {
    let fixture = Fixture::new();
    // Ends 2026-09-11T21:00:00Z; 71 entries reach back roughly 12 days.
    let files = write_block_chain(&fixture, 1_789_160_400_000, 71);

    assert_output_ignores_mtimes(
        &fixture,
        &files,
        FRESH_MTIME,
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
        str::to_string,
    );
}

#[test]
fn statusline_active_block_keeps_anchors_from_older_files() {
    let fixture = Fixture::new();
    let now_ms = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let files = write_block_chain(&fixture, now_ms - 60 * 60 * 1000, 71);
    let hook = format!(
        r#"{{"session_id":"session-chain","transcript_path":"{}","model":{{"display_name":"Model"}},"cost":{{"total_cost_usd":0}},"context_window":{{"total_input_tokens":1,"context_window_size":100}}}}"#,
        files.last().unwrap().display()
    );
    let fresh_mtime = u64::try_from(now_ms / 1000).unwrap();

    let block = assert_output_ignores_mtimes(
        &fixture,
        &files,
        fresh_mtime,
        &[
            "statusline",
            "--offline",
            "--no-cache",
            "--cost-source",
            "cc",
            "--timezone",
            "UTC",
        ],
        Some(&hook),
        // Keep only the block cost; the remaining time moves with the clock.
        |output| {
            output
                .split(" / ")
                .find(|segment| segment.contains("block"))
                .and_then(|segment| segment.split(" (").next())
                .unwrap_or(output)
                .to_string()
        },
    );
    assert_eq!(block, "$1.00 block");
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
    // 2026-09-01T12:00:00Z
    set_file_modified(&old, 1_788_264_000);

    let output = assert_output_ignores_mtimes(
        &fixture,
        &[old],
        FRESH_MTIME,
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
        str::to_string,
    );
    assert!(output.contains("Detected: Claude"), "{output}");
}

#[test]
fn statusline_keeps_the_current_block_before_future_dated_entries() {
    let fixture = Fixture::new();
    let now_ms = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let at = |offset_hours: i64| {
        ccusage_core::format_rfc3339_millis(ccusage_core::TimestampMs::from_millis(
            now_ms + offset_hours * 60 * 60 * 1000,
        ))
    };
    // A skewed clock wrote an entry eight hours ahead, after a long pause.
    let transcript = fixture.write_file(
        "projects/project-a/session-now.jsonl",
        [
            usage_line(&at(-1), "session-now", "msg-now", "req-now", 1.0),
            usage_line(&at(8), "session-now", "msg-future", "req-future", 9.0),
        ]
        .join("\n"),
    );
    let hook = format!(
        r#"{{"session_id":"session-now","transcript_path":"{}","model":{{"display_name":"Model"}},"cost":{{"total_cost_usd":0}},"context_window":{{"total_input_tokens":1,"context_window_size":100}}}}"#,
        transcript.display()
    );

    let output = run_ccusage(
        &fixture,
        &[
            "statusline",
            "--offline",
            "--no-cache",
            "--cost-source",
            "cc",
            "--timezone",
            "UTC",
        ],
        Some(&hook),
    );
    assert!(output.contains("$1.00 block"), "{output}");
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
    // Workflow transcripts can live under one session and record another.
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
    // 2026-09-01T12:00:00Z and 2026-09-11T12:05:00Z
    set_file_modified(&parent, 1_788_264_000);
    set_file_modified(&workflow, 1_789_128_300);

    let output = assert_output_ignores_mtimes(
        &fixture,
        &[parent, workflow],
        FRESH_MTIME,
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
        str::to_string,
    );
    let json: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(json["totals"]["totalCost"], 1.0, "{output}");
}
