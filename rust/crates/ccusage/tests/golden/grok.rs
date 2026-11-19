use ccusage_test_support::instant;
use std::{ffi::OsString, path::Path};

const GROK_45: &str = "grok-4.5-build";
const CODE_FAST: &str = "grok-code-fast-1";
const UNPRICED: &str = "grok-mystery-1";

const ALPHA: &str = "grok/sessions/%2Fwork%2Falpha";
const BETA: &str = "grok/sessions/%2Fwork%2Fbeta";

fn update(
    kind: &str,
    envelope: Option<&str>,
    session: Option<&str>,
    usage: Option<&str>,
    meta: Option<&str>,
) -> String {
    let envelope = envelope.map_or_else(String::new, |value| format!(r#""timestamp":{value},"#));
    let session = session.map_or_else(String::new, |id| format!(r#""sessionId":"{id}","#));
    let usage = usage.map_or_else(String::new, |body| format!(r#","usage":{{{body}}}"#));
    let meta = meta.map_or_else(String::new, |body| format!(r#","_meta":{{{body}}}"#));
    format!(
        r#"{{{envelope}"method":"_x.ai/session/update","params":{{{session}"update":{{"sessionUpdate":"{kind}"{usage}}}{meta}}}}}"#
    )
}

fn turn(seconds: i64, session: Option<&str>, event_id: &str, usage: &str) -> String {
    update(
        "turn_completed",
        Some(&seconds.to_string()),
        session,
        Some(usage),
        Some(&format!(r#""eventId":"{event_id}""#)),
    )
}

fn model_usage(rows: &[(&str, &str)]) -> String {
    let rows: Vec<_> = rows
        .iter()
        .map(|(model, body)| format!(r#""{model}":{{{body}}}"#))
        .collect();
    format!(r#""modelUsage":{{{}}}"#, rows.join(","))
}

pub(crate) fn seed(root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
    let dec_27_2200 = instant("2025-12-27T22:00:00Z").secs();
    let dec_30_1000 = instant("2025-12-30T10:00:00Z").secs();
    let dec_30_1030 = instant("2025-12-30T10:30:00Z").secs();
    let dec_31_1530 = instant("2025-12-31T15:30:00Z").secs();
    let dec_31_1600 = instant("2025-12-31T16:00:00Z").secs();
    let dec_31_2350 = instant("2025-12-31T23:50:00Z").secs();
    let jan_01_0010 = instant("2026-01-01T00:10:00Z").secs();
    let jan_01_1200 = instant("2026-01-01T12:00:00Z").secs();
    let jan_01_1230 = instant("2026-01-01T12:30:00Z").secs();
    let jan_02_1000 = instant("2026-01-02T10:00:00Z").secs();
    let jan_02_1100 = instant("2026-01-02T11:00:00Z").secs();
    let write = |relative: &str, contents: &str| {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("fixture file has a parent"))
            .expect("failed to create fixture directory");
        std::fs::write(&path, contents).expect("failed to write fixture file");
    };
    let write_lines = |relative: &str, lines: &[String]| write(relative, &lines.join("\n"));

    let alpha = "sess-alpha";
    let gamma = "line-session-gamma";
    let never_counted = r#""inputTokens":9999,"outputTokens":9999"#;

    let session_a = format!("{ALPHA}/aaaaaaaa-0000-4000-8000-000000000001");
    let multi_model_turn = update(
        "turn_completed",
        Some(&jan_02_1000.to_string()),
        Some(alpha),
        Some(&format!(
            r#"{never_counted},"costUsdTicks":9999000000,{}"#,
            model_usage(&[
                (
                    GROK_45,
                    r#""inputTokens":2000,"outputTokens":300,"cachedReadTokens":500,"cacheCreationTokens":100,"reasoningTokens":80,"totalTokens":2300,"costUsdTicks":123000000"#
                ),
                (
                    CODE_FAST,
                    r#""inputTokens":800,"outputTokens":120,"cachedReadTokens":0,"reasoningTokens":0,"totalTokens":920"#
                ),
            ])
        )),
        Some(&format!(
            r#""eventId":"evt-a2","agentTimestampMs":{}"#,
            dec_30_1000 * 1000
        )),
    );
    write_lines(
        &format!("{session_a}/updates.jsonl"),
        &[
            update(
                "tool_call",
                Some(&dec_30_1000.to_string()),
                Some(alpha),
                Some(never_counted),
                Some(r#""eventId":"evt-tool""#),
            ),
            update(
                "agent_message_chunk",
                Some(&dec_30_1000.to_string()),
                Some(alpha),
                Some(&format!(r#"{never_counted},"note":"turn_completed""#)),
                Some(r#""eventId":"evt-chunk""#),
            ),
            turn(
                dec_27_2200,
                Some(alpha),
                "evt-a1",
                &model_usage(&[(
                    GROK_45,
                    r#""inputTokens":1000,"outputTokens":200,"cachedReadTokens":400,"reasoningTokens":50,"totalTokens":1200,"costUsdTicks":50000000"#,
                )]),
            ),
            multi_model_turn.clone(),
            multi_model_turn,
            turn(
                dec_31_1530,
                Some(alpha),
                "evt-a3",
                &model_usage(&[(
                    GROK_45,
                    r#""inputTokens":3000,"outputTokens":500,"cachedReadTokens":1000,"reasoningTokens":200,"costUsdTicks":2000000000"#,
                )]),
            ),
            turn(
                dec_31_1600,
                Some(alpha),
                "evt-a3",
                &model_usage(&[(GROK_45, r#""inputTokens":7777,"outputTokens":7777"#)]),
            ),
            turn(
                dec_31_1600,
                Some(alpha),
                "evt-a3",
                &model_usage(&[(UNPRICED, r#""inputTokens":300,"outputTokens":70"#)]),
            ),
            turn(
                dec_31_1600,
                Some(alpha),
                "evt-a4",
                &model_usage(&[("grok-4.7-build", r#""reasoningTokens":42"#)]),
            ),
            turn(
                jan_01_1200,
                Some(alpha),
                "evt-a5",
                &model_usage(&[(
                    GROK_45,
                    r#""inputTokens":250000,"outputTokens":1000,"cachedReadTokens":50000,"costUsdTicks":4000000000"#,
                )]),
            ),
            turn(
                jan_01_1230,
                Some(alpha),
                "evt-a6",
                &model_usage(&[(
                    CODE_FAST,
                    r#""inputTokens":"9999","outputTokens":40,"cachedReadTokens":-5,"cacheCreationTokens":7777.5,"costUsdTicks":"9999""#,
                )]),
            ),
            update(
                "turn_completed",
                Some(&jan_01_1230.to_string()),
                Some(alpha),
                None,
                Some(r#""eventId":"evt-no-usage""#),
            ),
            turn(
                jan_01_1230,
                Some(alpha),
                "evt-zero",
                &model_usage(&[(
                    GROK_45,
                    r#""inputTokens":0,"outputTokens":0,"costUsdTicks":9999000000"#,
                )]),
            ),
            format!(
                r#"{{"timestamp":{jan_01_1230},"params":{{"sessionId":"{alpha}","update":{{"sessionUpdate":"turn_completed","usage":{{"inputTokens":9999,"outp"#
            ),
        ],
    );
    write_lines(
        &format!("{session_a}/events.jsonl"),
        &[turn(
            dec_31_1530,
            Some(alpha),
            "evt-events-file",
            &model_usage(&[(GROK_45, never_counted)]),
        )],
    );
    write_lines(
        &format!("{session_a}/updates.jsonl.bak"),
        &[turn(
            dec_31_1530,
            Some(alpha),
            "evt-backup-file",
            &model_usage(&[(GROK_45, never_counted)]),
        )],
    );

    let session_b = format!("{BETA}/bbbbbbbb-0000-4000-8000-000000000002");
    write(
        &format!("{session_b}/summary.json"),
        r#"{"info":{"id":"summary-session-beta","cwd":"/summary/beta"},"git_root_dir":"/should/not/win","current_model_id":"grok-code-fast-1","unknown_field":true}"#,
    );
    let turn_without_event_id = |reasoning: u64, meta: Option<&str>| {
        update(
            "turn_completed",
            Some(&jan_01_1200.to_string()),
            Some(gamma),
            Some(&model_usage(&[(
                UNPRICED,
                &format!(r#""inputTokens":300,"outputTokens":70,"reasoningTokens":{reasoning}"#),
            )])),
            meta,
        )
    };
    write_lines(
        &format!("{session_b}/updates.jsonl"),
        &[
            turn(
                dec_31_2350,
                None,
                "evt-b1",
                r#""inputTokens":900,"outputTokens":400,"cachedReadTokens":700,"cacheCreationTokens":50,"reasoningTokens":30,"totalTokens":1300,"costUsdTicks":30000000"#,
            ),
            turn(
                jan_01_0010,
                Some(gamma),
                "evt-b2",
                &model_usage(&[(
                    "grok-4.3-build",
                    r#""inputTokens":600,"outputTokens":100,"cachedReadTokens":3000"#,
                )]),
            ),
            turn_without_event_id(10, None),
            turn_without_event_id(10, Some(r#""eventId":"""#)),
            turn_without_event_id(20, None),
            turn(
                jan_02_1000,
                None,
                "evt-b4",
                &model_usage(&[(
                    "claude-sonnet-4-20250514",
                    r#""inputTokens":800,"outputTokens":300,"cachedReadTokens":100,"cacheCreationTokens":5000,"costUsdTicks":10000000"#,
                )]),
            ),
            update(
                "turn_completed",
                Some(&format!("{jan_02_1100}.9")),
                None,
                Some(&model_usage(&[(
                    CODE_FAST,
                    r#""inputTokens":500,"outputTokens":50"#,
                )])),
                Some(&format!(
                    r#""eventId":"evt-b5","agentTimestampMs":"{}""#,
                    dec_30_1000 * 1000
                )),
            ),
            update(
                "turn_completed",
                None,
                None,
                Some(&model_usage(&[(
                    CODE_FAST,
                    r#""inputTokens":11,"outputTokens":1"#,
                )])),
                Some(r#""eventId":"evt-b6","agentTimestampMs":0"#),
            ),
        ],
    );

    let session_c = format!("{BETA}/cccccccc-0000-4000-8000-000000000003");
    write(
        &format!("{session_c}/summary.json"),
        r#"{"info":{"id":"never-used","cwd":"/never/us"#,
    );
    write_lines(
        &format!("{session_c}/updates.jsonl"),
        &[
            turn(
                dec_31_1530,
                Some(alpha),
                "evt-a3",
                &model_usage(&[(GROK_45, r#""inputTokens":8888,"outputTokens":8888"#)]),
            ),
            turn_without_event_id(10, None),
            turn(
                jan_01_1230,
                None,
                "evt-c3",
                r#""inputTokens":345,"outputTokens":55"#,
            ),
        ],
    );

    let session_d = "grok/sessions/plain-project/dddddddd-0000-4000-8000-000000000004";
    write(
        &format!("{session_d}/summary.json"),
        r#"{"info":{"id":"","cwd":""},"git_root_dir":"/git/delta"}"#,
    );
    write_lines(
        &format!("{session_d}/updates.jsonl"),
        &[turn(
            jan_02_1100,
            None,
            "evt-d1",
            &model_usage(&[(
                GROK_45,
                r#""inputTokens":400,"outputTokens":60,"cachedReadTokens":100"#,
            )]),
        )],
    );

    write_lines(
        &format!("{ALPHA}/nested/deeper/eeeeeeee-0000-4000-8000-000000000005/updates.jsonl"),
        &[turn(
            dec_30_1030,
            None,
            "evt-e1",
            &model_usage(&[(CODE_FAST, r#""inputTokens":120,"outputTokens":30"#)]),
        )],
    );

    write(
        &format!("{ALPHA}/ffffffff-0000-4000-8000-000000000006/updates.jsonl"),
        "",
    );
    write(
        &format!("{ALPHA}/99999999-0000-4000-8000-000000000007/updates.jsonl"),
        "\0\0\0not-jsonl turn_completed\n{broken",
    );
    write_lines(
        "grok/logs/unified.jsonl",
        &[turn(
            dec_31_1530,
            Some(alpha),
            "evt-unified-log",
            &model_usage(&[(GROK_45, never_counted)]),
        )],
    );

    vec![("GROK_HOME", vec![root.join("grok").into_os_string()])]
}
