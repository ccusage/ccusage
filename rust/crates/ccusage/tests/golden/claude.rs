use std::ffi::OsString;
use std::path::Path;

use ccusage_test_support::{Instant, instant, set_modified};

use crate::golden::{Case, json_path};

const SONNET: &str = "claude-sonnet-4-20250514";
const OPUS: &str = "claude-opus-4-20250514";
const HAIKU: &str = "claude-haiku-4-5-20251001";

const ALPHA: &str = "claude/projects/-Users-dev-alpha";
const BETA: &str = "claude/projects/-Users-dev-beta";
const GAMMA: &str = "claude/projects/-Users-dev-gamma";

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

pub(crate) fn seed(root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
    let write = |relative: &str, lines: &[String], modified: Instant| {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("fixture file has a parent"))
            .expect("failed to create fixture directory");
        std::fs::write(&path, lines.join("\n")).expect("failed to write fixture file");
        set_modified(&path, modified);
    };

    let alpha = "sess-alpha-1";
    let copied_turn = |session: &str| {
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
            entry(
                "2025-12-31T09:00:00.000Z",
                alpha,
                None,
                "msg_a4",
                SONNET,
                r#""input_tokens":120,"output_tokens":10,"cache_read_input_tokens":2000"#,
                r#","uuid":"a4-text","parentUuid":null"#,
            ),
            entry(
                "2025-12-31T09:00:02.000Z",
                alpha,
                None,
                "msg_a4",
                SONNET,
                r#""input_tokens":120,"output_tokens":340,"cache_read_input_tokens":2000"#,
                r#","uuid":"a4-tool","parentUuid":"a4-text""#,
            ),
            copied_turn(alpha),
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
                &format!(
                    r#","isApiErrorMessage":true,"content":[{{"type":"text","text":"Claude AI usage limit reached|{reset_at}"}}]"#,
                    reset_at = instant("2026-01-02T09:00:00Z").secs()
                ),
            ),
        ],
        instant("2026-01-02T08:31:00Z"),
    );

    write(
        &format!("{ALPHA}/{alpha}/subagents/agent-btw.jsonl"),
        &[
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
        instant("2025-12-30T10:08:00Z"),
    );

    write(
        &format!("{ALPHA}/{alpha}/subagents/workflows/wf-golden/agent-run.jsonl"),
        &[
            entry(
                "2025-12-30T10:00:00.000Z",
                alpha,
                Some("req-workflow-replay"),
                "msg_a1",
                SONNET,
                r#""input_tokens":1000,"output_tokens":200,"cache_creation_input_tokens":300,"cache_read_input_tokens":4000"#,
                r#","isSidechain":true"#,
            ),
            entry(
                "2025-12-31T09:30:00.000Z",
                alpha,
                Some("req-workflow-own"),
                "msg_workflow_own",
                HAIKU,
                r#""input_tokens":80,"output_tokens":25,"cache_read_input_tokens":150"#,
                r#","isSidechain":true,"costUSD":0.007"#,
            ),
        ],
        instant("2025-12-31T09:31:00Z"),
    );

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
        instant("2026-01-02T10:01:00Z"),
    );

    let resumed = "sess-beta-2";
    write(
        &format!("{BETA}/{resumed}.jsonl"),
        &[
            copied_turn(resumed),
            entry(
                "2026-01-01T03:00:00.000Z",
                resumed,
                Some("req-c0"),
                "msg_c0",
                SONNET,
                r#""input_tokens":4,"output_tokens":50,"cache_creation_input_tokens":1250,"cache_read_input_tokens":3000,"cache_creation":{"ephemeral_1h_input_tokens":1000,"ephemeral_5m_input_tokens":0},"iterations":[{"type":"message","input_tokens":2,"output_tokens":20,"cache_read_input_tokens":1000,"cache_creation_input_tokens":1000,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":1000}},{"type":"advisor_message","model":"claude-opus-5","input_tokens":2000,"output_tokens":300,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},{"type":"message","input_tokens":2,"output_tokens":30,"cache_read_input_tokens":2000,"cache_creation_input_tokens":250,"cache_creation":{"ephemeral_5m_input_tokens":250,"ephemeral_1h_input_tokens":0}}]"#,
                "",
            ),
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
        instant("2026-01-02T11:01:00Z"),
    );

    write(
        &format!("{BETA}/sess-empty.jsonl"),
        &[],
        instant("2025-12-30T00:00:00Z"),
    );

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
        instant("2025-12-27T22:01:00Z"),
    );

    vec![(
        "CLAUDE_CONFIG_DIR",
        vec![root.join("claude").into_os_string()],
    )]
}

pub(crate) fn extra_cases(root: &Path) -> Vec<Case> {
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
            "session",
            "workflow_parent",
            &[
                "claude",
                "session",
                "--offline",
                "--json",
                "--id",
                "sess-alpha-1",
                "--mode",
                "display",
            ],
        ),
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
            "json_order_asc",
            &["claude", "session", "--offline", "--json", "--order", "asc"],
        ),
        Case::new(
            "session",
            "table_order_asc",
            &["claude", "session", "--offline", "--order", "asc"],
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
            "id_json_order_desc",
            &[
                "claude",
                "session",
                "--offline",
                "--json",
                "--id",
                "sess-alpha-1",
                "--order",
                "desc",
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
            "table_token_limit_narrow",
            &[
                "claude",
                "blocks",
                "--offline",
                "--no-color",
                "--since",
                "20260102",
                "--token-limit",
                "100",
            ],
        )
        .columns(80),
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

#[test]
fn session_order_honors_flags_and_config_without_changing_the_default() {
    use ccusage_test_support::Fixture;
    use serde_json::Value;

    use crate::golden::run_case;

    let fixture = Fixture::new();
    let seeded = seed(fixture.root());
    let config = fixture.write_file("order.json", r#"{"defaults":{"order":"asc"}}"#);
    let report = |flags: &[&str]| {
        let mut case = Case::new("session", "", &["claude", "session", "--offline", "--json"]);
        case.args
            .extend(flags.iter().map(|flag| (*flag).to_string()));
        let output = run_case(fixture.root(), &seeded, &case);
        assert_eq!(output.exit, "0", "{}", output.stderr);
        output.stdout
    };
    let ids = |output: &str| {
        serde_json::from_str::<Value>(output).unwrap()["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["sessionId"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    let ascending = [
        "sess-gamma-old",
        "sess-beta-2",
        "sess-alpha-1",
        "sess-beta-1",
    ];
    assert_eq!(ids(&report(&["--order", "asc"])), ascending);
    assert_eq!(
        ids(&report(&["--config", config.to_str().unwrap()])),
        ascending
    );
    assert_eq!(report(&[]), report(&["--order", "desc"]));
    assert_eq!(
        report(&["--config", config.to_str().unwrap(), "--order", "desc"]),
        report(&[]),
    );
}

#[test]
fn chronological_reports_honor_order_without_changing_their_default() {
    use ccusage_test_support::Fixture;
    use serde_json::Value;

    use crate::golden::run_case;

    let fixture = Fixture::new();
    let seeded = seed(fixture.root());
    let config = fixture.write_file("order.json", r#"{"defaults":{"order":"desc"}}"#);
    for (kind, field) in [
        ("daily", "date"),
        ("weekly", "week"),
        ("monthly", "month"),
        ("blocks", "startTime"),
    ] {
        let report = |flags: &[&str]| {
            let mut case = Case::new(kind, "", &["claude", kind, "--offline", "--json"]);
            case.args
                .extend(flags.iter().map(|flag| (*flag).to_string()));
            let output = run_case(fixture.root(), &seeded, &case);
            assert_eq!(output.exit, "0", "{kind}: {}", output.stderr);
            output.stdout
        };
        let periods = |output: &str| {
            serde_json::from_str::<Value>(output).unwrap()[kind]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row[field].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        };
        let ascending = report(&["--order", "asc"]);
        assert_eq!(report(&[]), ascending, "{kind}");
        let mut expected = periods(&ascending);
        assert!(expected.len() > 1, "{kind}");
        assert!(expected.windows(2).all(|pair| pair[0] < pair[1]), "{kind}");
        expected.reverse();
        assert_eq!(periods(&report(&["--order", "desc"])), expected, "{kind}");
        assert_eq!(
            periods(&report(&["--config", config.to_str().unwrap()])),
            expected,
            "{kind}",
        );
    }
}

#[test]
fn session_id_orders_entries_without_reordering_aggregation() {
    use ccusage_test_support::Fixture;
    use serde_json::Value;

    use crate::golden::run_case;

    let fixture = Fixture::new();
    let seeded = seed(fixture.root());
    let config = fixture.write_file("order.json", r#"{"defaults":{"order":"desc"}}"#);
    let report = |flags: &[&str]| {
        let mut case = Case::new(
            "session",
            "",
            &[
                "claude",
                "session",
                "--offline",
                "--json",
                "--id",
                "sess-alpha-1",
            ],
        );
        case.args
            .extend(flags.iter().map(|flag| (*flag).to_string()));
        let output = run_case(fixture.root(), &seeded, &case);
        assert_eq!(output.exit, "0", "{}", output.stderr);
        output.stdout
    };
    let default = report(&[]);
    assert_eq!(default, report(&["--order", "asc"]));
    let mut expected: Value = serde_json::from_str(&default).unwrap();
    expected.as_object_mut().unwrap().remove("entries");
    for flags in [
        vec!["--order", "desc"],
        vec!["--config", config.to_str().unwrap()],
    ] {
        let mut actual: Value = serde_json::from_str(&report(&flags)).unwrap();
        let inputs = actual["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["inputTokens"].as_u64().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(inputs, [0, 200, 300, 400, 250, 700, 80, 120, 50, 500, 1000]);
        actual.as_object_mut().unwrap().remove("entries");
        assert_eq!(actual, expected);
    }
}
