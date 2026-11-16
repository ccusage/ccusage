use std::path::{Path, PathBuf};

use crate::golden::{Case, json_path, set_modified};

const SONNET: &str = "claude-sonnet-4-20250514";
const OPUS: &str = "claude-opus-4-20250514";
const HAIKU: &str = "claude-haiku-4-5-20251001";

const ALPHA: &str = "claude/projects/-Users-dev-alpha";
const BETA: &str = "claude/projects/-Users-dev-beta";
const GAMMA: &str = "claude/projects/-Users-dev-gamma";

/// One assistant transcript line. `usage` is the body of the usage object and
/// `extra` holds further top-level members, each with its leading comma.
fn entry(
    timestamp: &str,
    session: &str,
    request_id: Option<&str>,
    message_id: &str,
    model: &str,
    usage: &str,
    extra: &str,
) -> String {
    let request = request_id.map_or_else(String::new, |id| format!(r#""requestId":"{id}","#));
    format!(
        r#"{{"type":"assistant","timestamp":"{timestamp}","sessionId":"{session}","version":"2.0.14",{request}"message":{{"role":"assistant","id":"{message_id}","model":"{model}","usage":{{{usage}}}}}{extra}}}"#
    )
}

pub fn seed(root: &Path) -> Vec<(&'static str, PathBuf)> {
    let write = |relative: &str, lines: &[String], modified: u64| {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("fixture file has a parent"))
            .expect("failed to create fixture directory");
        std::fs::write(&path, lines.join("\n")).expect("failed to write fixture file");
        set_modified(&path, modified);
    };

    let alpha = "sess-alpha-1";
    let copied_turn = |session: &str| {
        // 2026-01-01 in Asia/Tokyo. Written to two transcripts with the same
        // message and request IDs, as a resumed session does.
        entry(
            "2025-12-31T15:30:00.000Z",
            session,
            Some("req-a3"),
            "msg_a3",
            SONNET,
            r#""input_tokens":700,"output_tokens":150,"cache_creation_input_tokens":100,"cache_read_input_tokens":5000"#,
            r#","costUSD":0.02"#,
        )
    };

    // Flat transcript: the session ID is the file stem.
    write(
        &format!("{ALPHA}/{alpha}.jsonl"),
        &[
            r#"{"type":"summary","summary":"Golden fixture session","leafUuid":"leaf-1"}"#
                .to_string(),
            format!(
                r#"{{"type":"user","timestamp":"2025-12-30T09:59:00.000Z","sessionId":"{alpha}","message":{{"role":"user","content":"hello"}}}}"#
            ),
            entry(
                "2025-12-30T10:00:00.000Z",
                alpha,
                Some("req-a1"),
                "msg_a1",
                SONNET,
                r#""input_tokens":1000,"output_tokens":200,"cache_creation_input_tokens":300,"cache_read_input_tokens":4000"#,
                r#","costUSD":0.05"#,
            ),
            entry(
                "2025-12-30T10:05:00.000Z",
                alpha,
                Some("req-a2"),
                "msg_a2",
                OPUS,
                r#""input_tokens":500,"output_tokens":800,"cache_creation_input_tokens":600,"cache_read_input_tokens":9000,"cache_creation":{"ephemeral_5m_input_tokens":200,"ephemeral_1h_input_tokens":400}"#,
                "",
            ),
            // One response written once per content block: same `msg_` ID, no
            // request ID, distinct timestamps.
            entry(
                "2025-12-31T09:00:00.000Z",
                alpha,
                None,
                "msg_a4",
                SONNET,
                r#""input_tokens":120,"output_tokens":10,"cache_read_input_tokens":2000"#,
                "",
            ),
            entry(
                "2025-12-31T09:00:02.000Z",
                alpha,
                None,
                "msg_a4",
                SONNET,
                r#""input_tokens":120,"output_tokens":340,"cache_read_input_tokens":2000"#,
                "",
            ),
            copied_turn(alpha),
            // Carries the `"usage":{` marker the loader scans for, but is not a
            // usage entry.
            format!(
                r#"{{"type":"user","timestamp":"2025-12-31T15:31:00.000Z","sessionId":"{alpha}","toolUseResult":{{"usage":{{"input_tokens":7777,"output_tokens":7777}}}}}}"#
            ),
            entry(
                "2026-01-01T12:00:00.000Z",
                alpha,
                Some("req-a5"),
                "msg_a5",
                SONNET,
                &format!(
                    r#""input_tokens":400,"output_tokens":90,"cache_read_input_tokens":1000,"iterations":[{{"type":"message","model":null,"input_tokens":400,"output_tokens":90}},{{"type":"advisor_message","model":"{OPUS}","input_tokens":250,"output_tokens":60,"cache_read_input_tokens":500}}]"#
                ),
                "",
            ),
            entry(
                "2026-01-01T12:30:00.000Z",
                alpha,
                Some("req-a6"),
                "msg_a6",
                "mystery-model-9000",
                r#""input_tokens":300,"output_tokens":70,"cache_read_input_tokens":800"#,
                "",
            ),
            format!(
                r#"{{"type":"assistant","timestamp":"2026-01-01T13:00:00.000Z","sessionId":"{alpha}","requestId":"req-trunc","message":{{"id":"msg_trunc","model":"{SONNET}","usage":{{"input_tokens":9999,"outp"#
            ),
            entry(
                "2026-01-02T08:00:00.000Z",
                alpha,
                Some("req-a7"),
                "msg_a7",
                "claude-opus-5",
                r#""input_tokens":200,"output_tokens":50,"cache_read_input_tokens":300,"speed":"fast""#,
                "",
            ),
            entry(
                "2026-01-02T08:30:00.000Z",
                alpha,
                Some("req-a8"),
                "msg_a8",
                "<synthetic>",
                r#""input_tokens":0,"output_tokens":0"#,
                r#","isApiErrorMessage":true,"content":[{"type":"text","text":"Claude AI usage limit reached|1767344400"}]"#,
            ),
        ],
        1_767_342_660, // 2026-01-02T08:31:00Z
    );

    // Subagent transcript: the session ID is the directory above `subagents/`.
    write(
        &format!("{ALPHA}/{alpha}/subagents/agent-btw.jsonl"),
        &[
            // Replays of parent messages under new request IDs: the first at
            // the parent's timestamp, the second a minute after it.
            entry(
                "2025-12-30T10:00:00.000Z",
                alpha,
                Some("req-btw-1"),
                "msg_a1",
                SONNET,
                r#""input_tokens":1000,"output_tokens":200,"cache_creation_input_tokens":300,"cache_read_input_tokens":4000"#,
                r#","isSidechain":true"#,
            ),
            entry(
                "2025-12-30T10:06:00.000Z",
                alpha,
                Some("req-btw-2"),
                "msg_a2",
                OPUS,
                r#""input_tokens":500,"output_tokens":800,"cache_creation_input_tokens":600,"cache_read_input_tokens":9000"#,
                r#","isSidechain":true"#,
            ),
            entry(
                "2025-12-30T10:07:00.000Z",
                alpha,
                Some("req-btw-3"),
                "msg_btw_own",
                HAIKU,
                r#""input_tokens":50,"output_tokens":20,"cache_read_input_tokens":100"#,
                r#","isSidechain":true"#,
            ),
        ],
        1_767_089_280, // 2025-12-30T10:08:00Z
    );

    // Nested transcript: the session ID is the directory name.
    let beta = "sess-beta-1";
    let gateway = |timestamp: &str| {
        entry(
            timestamp,
            beta,
            None,
            "gw-1",
            SONNET,
            r#""input_tokens":10,"output_tokens":5"#,
            "",
        )
    };
    write(
        &format!("{BETA}/{beta}/chat.jsonl"),
        &[
            // 2026-01-01 in Asia/Tokyo.
            entry(
                "2025-12-31T23:50:00.000Z",
                beta,
                Some("req-b1"),
                "msg_b1",
                OPUS,
                r#""input_tokens":900,"output_tokens":400,"cache_creation_input_tokens":200,"cache_read_input_tokens":7000"#,
                "",
            ),
            entry(
                "2026-01-01T00:10:00.000Z",
                beta,
                Some("req-b2"),
                "msg_b2",
                SONNET,
                r#""input_tokens":600,"output_tokens":100,"cache_read_input_tokens":3000"#,
                r#","costUSD":0.5"#,
            ),
            // A gateway placeholder ID reused per response: the repeat at the
            // same timestamp is one write, the later one is another response.
            gateway("2026-01-01T01:00:00.000Z"),
            gateway("2026-01-01T01:00:00.000Z"),
            gateway("2026-01-01T01:00:05.000Z"),
            format!(
                r#"{{"type":"progress","sessionId":"{beta}","data":{{"message":{{"timestamp":"2026-01-01T02:00:00.000Z","requestId":"req-b-wrapped","message":{{"id":"msg_b_wrapped","model":"{SONNET}","usage":{{"input_tokens":33,"output_tokens":44}}}}}}}}}}"#
            ),
            entry(
                "2026-01-02T10:00:00.000Z",
                beta,
                Some("req-b3"),
                "msg_b3",
                SONNET,
                r#""input_tokens":800,"output_tokens":300,"cache_creation_input_tokens":50,"cache_read_input_tokens":6000"#,
                "",
            ),
        ],
        1_767_348_060, // 2026-01-02T10:01:00Z
    );

    let resumed = "sess-beta-2";
    write(
        &format!("{BETA}/{resumed}.jsonl"),
        &[
            copied_turn(resumed),
            entry(
                "2026-01-02T11:00:00.000Z",
                resumed,
                Some("req-c1"),
                "msg_c1",
                OPUS,
                r#""input_tokens":150,"output_tokens":600,"cache_read_input_tokens":2500"#,
                "",
            ),
        ],
        1_767_351_660, // 2026-01-02T11:01:00Z
    );

    write(&format!("{BETA}/sess-empty.jsonl"), &[], 1_767_052_800); // 2025-12-30T00:00:00Z

    // Last written more than a day before the shared window opens, so the
    // window variants prune the file by mtime.
    write(
        &format!("{GAMMA}/sess-gamma-old.jsonl"),
        &[entry(
            "2025-12-27T22:00:00.000Z",
            "sess-gamma-old",
            Some("req-g1"),
            "msg_g1",
            SONNET,
            r#""input_tokens":100,"output_tokens":50"#,
            r#","costUSD":0.01"#,
        )],
        1_766_872_860, // 2025-12-27T22:01:00Z
    );

    vec![("CLAUDE_CONFIG_DIR", root.join("claude"))]
}

/// Claude-only options, plus `statusline`, which reads a hook payload from
/// stdin. The clock is pinned to `golden::FIXED_NOW`, inside the last block.
pub fn extra_cases(root: &Path) -> Vec<Case> {
    let transcript = root.join(ALPHA).join("sess-alpha-1.jsonl");
    let hook = |context: &str| {
        format!(
            r#"{{"session_id":"sess-alpha-1","transcript_path":{},"model":{{"id":"{SONNET}","display_name":"Sonnet 4"}},"cost":{{"total_cost_usd":1.23}},"effort":{{"level":"high"}}{context}}}"#,
            json_path(&transcript)
        )
    };
    let hook_with_context =
        hook(r#","context_window":{"total_input_tokens":50000,"context_window_size":200000}"#);

    vec![
        Case::new(
            "daily",
            "json_instances",
            &["claude", "daily", "--offline", "--json", "--instances"],
        ),
        Case::new(
            "daily",
            "table_instances",
            &["claude", "daily", "--offline", "--instances"],
        ),
        Case::new(
            "daily",
            "json_project",
            &[
                "claude",
                "daily",
                "--offline",
                "--json",
                "--project=-Users-dev-beta",
            ],
        ),
        Case::new(
            "weekly",
            "json_start_monday",
            &[
                "claude",
                "weekly",
                "--offline",
                "--json",
                "--start-of-week",
                "monday",
            ],
        ),
        Case::new(
            "session",
            "id_json",
            &[
                "claude",
                "session",
                "--offline",
                "--json",
                "--id",
                "sess-alpha-1",
            ],
        ),
        Case::new(
            "session",
            "id_table_breakdown",
            &[
                "claude",
                "session",
                "--offline",
                "--breakdown",
                "--id",
                "sess-alpha-1",
            ],
        ),
        Case::new(
            "session",
            "id_json_missing",
            &[
                "claude",
                "session",
                "--offline",
                "--json",
                "--id",
                "sess-missing",
            ],
        ),
        Case::new(
            "blocks",
            "json_token_limit_max",
            &[
                "claude",
                "blocks",
                "--offline",
                "--json",
                "--token-limit",
                "max",
            ],
        ),
        Case::new(
            "blocks",
            "json_session_length_1",
            &[
                "claude",
                "blocks",
                "--offline",
                "--json",
                "--session-length",
                "1",
            ],
        ),
        Case::new(
            "blocks",
            "json_recent",
            &["claude", "blocks", "--offline", "--json", "--recent"],
        ),
        Case::new(
            "blocks",
            "table_active",
            &["claude", "blocks", "--offline", "--active"],
        ),
        Case::new(
            "statusline",
            "cost_source_both",
            &[
                "claude",
                "statusline",
                "--offline",
                "--no-cache",
                "--cost-source",
                "both",
            ],
        )
        .stdin(hook_with_context.clone()),
        Case::new(
            "statusline",
            "cost_source_ccusage_transcript_context",
            &[
                "claude",
                "statusline",
                "--offline",
                "--no-cache",
                "--cost-source",
                "ccusage",
            ],
        )
        .stdin(hook("")),
        Case::new(
            "statusline",
            "tz_tokyo",
            &[
                "claude",
                "statusline",
                "--offline",
                "--no-cache",
                "--timezone",
                "Asia/Tokyo",
            ],
        )
        .stdin(hook_with_context),
        Case::new(
            "statusline",
            "no_input",
            &["claude", "statusline", "--offline", "--no-cache"],
        ),
    ]
}
