use std::ffi::OsString;
use std::path::Path;

use ccusage_test_support::instant;

use crate::golden::{Case, windowed_table};

const V4_PRO: &str = "deepseek-v4-pro";
const FLASH: &str = "deepseek-flash";
const SONNET: &str = "anthropic/claude-sonnet-4.5";
const UNPRICED: &str = "dsh-mystery-1";

fn header(created_at: i64, fields: &str) -> String {
    format!(r#"{{"type":"session","version":4,"createdAt":{created_at}{fields}}}"#)
}

fn event(kind: &str, seq: Option<&str>, time: Option<&str>, data: &str) -> String {
    let seq = seq.map_or_else(String::new, |seq| format!(r#","seq":{seq}"#));
    let time = time.map_or_else(String::new, |time| format!(r#","time":{time}"#));
    format!(r#"{{"type":"{kind}"{seq}{time},"data":{data}}}"#)
}

fn route(seq: i64, time: i64, provider: &str, model: &str) -> String {
    event(
        "request/header",
        Some(&seq.to_string()),
        Some(&time.to_string()),
        &format!(
            r#"{{"header":{{"config":{{"provider":"{provider}","model":"{model}"}},"tools":[]}},"reason":"initial"}}"#
        ),
    )
}

fn message(seq: i64, time: i64, usage: &str) -> String {
    event(
        "assistant/message",
        Some(&seq.to_string()),
        Some(&time.to_string()),
        &format!(
            r#"{{"turn":1,"step":1,"message":{{"role":"assistant","content":[]}},"stream":[],"usage":{{{usage}}}}}"#
        ),
    )
}

fn document(lines: &[String]) -> String {
    lines.join("\n")
}

fn seed_session_with_torn_zstd_frame(
    write: &impl Fn(&str, &[u8]),
    beta_parent_call: &str,
    never_counted: &str,
) {
    let beta_head = header(
        instant("2025-12-31T23:50:00Z").millis(),
        r#","id":"sess-beta","cwd":"/work/beta""#,
    );
    let beta_lines = [
        route(
            1,
            instant("2025-12-31T23:50:00Z").millis(),
            "deepseek-official",
            V4_PRO,
        ),
        beta_parent_call.to_owned(),
        message(
            3,
            instant("2026-01-02T10:00:00Z").millis(),
            r#""inputTokens":700,"outputTokens":90,"cacheWriteTokens":300"#,
        ),
    ];
    let mut beta = zstd::encode_all(format!("{beta_head}\n").as_bytes(), 0)
        .expect("failed to compress fixture");
    beta.extend(
        zstd::encode_all(format!("{}\n", beta_lines.join("\n")).as_bytes(), 0)
            .expect("failed to compress fixture"),
    );
    let torn = zstd::encode_all(
        message(4, instant("2026-01-02T10:00:00Z").millis(), never_counted).as_bytes(),
        0,
    )
    .expect("failed to compress fixture");
    beta.extend(&torn[..torn.len() / 2]);
    write(
        "home-a/sessions/--work-beta--/sess-beta/session.v4.jsonl.zstd",
        &beta,
    );
}

fn seed_fork_with_multiple_inherited_markers(write: &impl Fn(&str, &[u8]), beta_parent_call: &str) {
    write(
        "home-a/sessions/--work-beta--/fork-beta/session.v4.jsonl",
        document(&[
            header(
                instant("2026-01-02T08:00:00Z").millis(),
                r#","id":"fork-beta","cwd":"/work/beta","parentSession":"sess-beta","isSeeded":true"#,
            ),
            route(1, instant("2025-12-31T23:50:00Z").millis(), "deepseek-official", V4_PRO),
            beta_parent_call.to_owned(),
            event("session/end-seed", Some("3"), Some(&instant("2026-01-02T08:00:00Z").millis().to_string()), "{}"),
            event(
                "session/end-seed",
                Some("4"),
                Some(&instant("2026-01-02T08:00:00Z").millis().to_string()),
                r#"{"inherited":true}"#,
            ),
            message(5, instant("2026-01-02T08:00:00Z").millis() + 5_000, r#""inputTokens":600,"outputTokens":60"#),
            route(6, instant("2026-01-02T10:00:00Z").millis(), "deepseek-account", FLASH),
            message(
                7,
                instant("2026-01-02T10:00:00Z").millis(),
                r#""inputTokens":1500,"outputTokens":250,"cacheReadTokens":3000"#,
            ),
        ])
        .as_bytes(),
    );
}

fn seed_fork_torn_before_inherited_marker(write: &impl Fn(&str, &[u8]), beta_parent_call: String) {
    write(
        "home-a/sessions/--work-beta--/fork-torn/session.v4.jsonl",
        document(&[
            header(
                instant("2026-01-02T08:00:00Z").millis(),
                r#","id":"fork-torn","isSeeded":true"#,
            ),
            route(
                1,
                instant("2025-12-31T23:50:00Z").millis(),
                "deepseek-official",
                V4_PRO,
            ),
            beta_parent_call,
        ])
        .as_bytes(),
    );
}

pub(crate) fn seed(root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
    let write = |relative: &str, contents: &[u8]| {
        let path = root.join("dsh").join(relative);
        std::fs::create_dir_all(path.parent().expect("fixture file has a parent"))
            .expect("failed to create DeepSeek Harness fixture directory");
        std::fs::write(&path, contents).expect("failed to write DeepSeek Harness fixture file");
    };
    let never_counted = r#""inputTokens":9999,"outputTokens":9999"#;

    let alpha_header = header(
        instant("2025-12-30T09:50:00Z").millis(),
        r#","id":"sess-alpha","cwd":"/work/alpha","isSeeded":false,"delegationDepth":0"#,
    );
    let alpha_tokyo = message(
        5,
        instant("2025-12-31T15:30:00Z").millis(),
        r#""inputTokens":400,"outputTokens":120,"cacheReadTokens":9000,"cacheWriteTokens":600,"totalTokens":10120"#,
    );
    let alpha = vec![
        alpha_header.clone(),
        route(
            1,
            instant("2025-12-30T10:00:00Z").millis(),
            "deepseek-official",
            V4_PRO,
        ),
        event(
            "request/context",
            Some("2"),
            Some(&instant("2025-12-30T10:00:00Z").millis().to_string()),
            &format!(
                r#"{{"provider":"deepseek-official","model":"{V4_PRO}","contextWindow":1048576}}"#
            ),
        ),
        event(
            "assistant/message",
            Some("3"),
            Some(&(instant("2025-12-30T10:00:00Z").millis() + 5_000).to_string()),
            r#"{"stream":[{"type":"chunk","chunk":{"type":"usage","usage":{"inputTokens":9999}}}],"usage":{"inputTokens":1200,"outputTokens":300,"cacheReadTokens":8000,"totalTokens":9500}}"#,
        ),
        event(
            "assistant/attempt",
            Some("4"),
            Some(&instant("2025-12-31T10:00:00Z").millis().to_string()),
            r#"{"stream":[{"type":"chunk","chunk":{"type":"usage","usage":{"inputTokens":5,"outputTokens":0}}},{"type":"chunk","chunk":{"type":"text","text":"x"}},{"type":"chunk","chunk":{"type":"usage","usage":{"inputTokens":50,"outputTokens":3}}}]}"#,
        ),
        alpha_tokyo.clone(),
        alpha_tokyo,
        message(
            6,
            instant("2025-12-31T15:30:00Z").millis(),
            r#""inputTokens":400,"outputTokens":120,"cacheReadTokens":9000,"cacheWriteTokens":600,"totalTokens":10120"#,
        ),
        "not json but mentions \"usage\"".to_string(),
        event(
            "request/context",
            Some("7"),
            Some(&instant("2026-01-01T00:10:00Z").millis().to_string()),
            &format!(r#"{{"provider":"openrouter","model":"{SONNET}","contextWindow":200000}}"#),
        ),
        message(
            8,
            instant("2026-01-01T00:10:00Z").millis(),
            r#""inputTokens":300,"outputTokens":80,"cacheWriteTokens":200"#,
        ),
        event(
            "request/context",
            Some("9"),
            Some(&instant("2026-01-01T00:10:00Z").millis().to_string()),
            r#"{"provider":"anthropic"}"#,
        ),
        message(
            10,
            instant("2026-01-01T12:00:00Z").millis(),
            r#""totalTokens":432"#,
        ),
        message(
            11,
            instant("2026-01-01T12:00:00Z").millis() + 60_000,
            r#""inputTokens":100,"outputTokens":50,"totalTokens":400"#,
        ),
        message(
            12,
            instant("2026-01-01T12:00:00Z").millis() + 120_000,
            r#""inputTokens":0,"outputTokens":0"#,
        ),
        event(
            "compaction/summary",
            Some("13"),
            Some(&instant("2026-01-01T12:30:00Z").millis().to_string()),
            &format!(
                r#"{{"compactionId":"c1","provider":"deepseek-account","model":"{FLASH}","usage":{{"inputTokens":19000,"outputTokens":800,"totalTokens":19800}}}}"#
            ),
        ),
        event(
            "compaction/summary",
            Some("14"),
            Some(&instant("2026-01-01T12:30:00Z").millis().to_string()),
            r#"{"provider":"deepseek-account","usage":{"inputTokens":90,"outputTokens":10}}"#,
        ),
        event(
            "user/message",
            Some("15"),
            Some(&instant("2026-01-01T12:30:00Z").millis().to_string()),
            &format!("{{\"usage\":{{{never_counted}}}}}"),
        ),
        event(
            "assistant/message",
            Some("16"),
            Some(&instant("2026-01-01T12:30:00Z").millis().to_string()),
            "null",
        ),
        message(17, 0, r#""inputTokens":70,"outputTokens":30"#),
        event(
            "assistant/message",
            Some("18"),
            None,
            r#"{"usage":{"inputTokens":60,"outputTokens":20}}"#,
        ),
        event(
            "assistant/message",
            Some("19"),
            Some(r#""2026-01-01T12:00:00Z""#),
            r#"{"usage":{"inputTokens":50,"outputTokens":10}}"#,
        ),
        event(
            "assistant/message",
            Some("20"),
            Some(&instant("2026-01-01T12:30:00Z").millis().to_string()),
            r#"{"usage":{"inputTokens":"9999","outputTokens":7.5,"cacheReadTokens":-5,"cacheWriteTokens":40}}"#,
        ),
        r#"{"type":"assistant/message","seq":21,"data":{"usage":{"inputTokens":9999,"#.to_string(),
    ];
    write(
        "home-a/sessions/--work-alpha--/sess-alpha/session.v4.jsonl",
        document(&alpha).as_bytes(),
    );
    write(
        "home-a/sessions/--work-alpha--/sess-alpha/session.v3.jsonl",
        document(&[
            alpha_header.clone(),
            route(
                1,
                instant("2025-12-30T10:00:00Z").millis(),
                "deepseek-official",
                V4_PRO,
            ),
            message(90, instant("2025-12-30T10:00:00Z").millis(), never_counted),
        ])
        .as_bytes(),
    );
    write(
        "home-a/sessions/--work-alpha--/sess-alpha/session.lock",
        b"locked",
    );

    let mut alpha_copy = alpha[..alpha.len() - 1].to_vec();
    alpha_copy.push(message(
        30,
        instant("2026-01-02T08:00:00Z").millis(),
        r#""inputTokens":800,"outputTokens":200,"cacheReadTokens":4000"#,
    ));
    write(
        "home-b/sessions/--work-alpha--/sess-alpha/session.v4.jsonl",
        document(&alpha_copy).as_bytes(),
    );

    let beta_parent_call = message(
        2,
        instant("2025-12-31T23:50:00Z").millis(),
        r#""inputTokens":2000,"outputTokens":500,"cacheReadTokens":6000,"totalTokens":8500"#,
    );
    seed_session_with_torn_zstd_frame(&write, &beta_parent_call, never_counted);

    seed_fork_with_multiple_inherited_markers(&write, &beta_parent_call);

    seed_fork_torn_before_inherited_marker(&write, beta_parent_call);

    let no_seq = event(
        "assistant/message",
        None,
        None,
        r#"{"usage":{"inputTokens":250,"outputTokens":40}}"#,
    );
    write(
        "home-a/sessions/_no-cwd/sess-legacy/session.jsonl",
        document(&[
            header(instant("2025-12-27T22:00:00Z").millis(), ""),
            no_seq.clone(),
            no_seq,
            event(
                "assistant/message",
                None,
                None,
                r#"{"usage":{"inputTokens":250,"outputTokens":41}}"#,
            ),
        ])
        .as_bytes(),
    );

    write(
        "home-a/sessions/deep/a/b/sess-deep/session.v2.jsonl",
        document(&[
            header(
                instant("2025-12-31T10:00:00Z").millis(),
                r#","id":"sess-deep","cwd":"/work/deep""#,
            ),
            route(
                1,
                instant("2025-12-31T10:00:00Z").millis(),
                "deepseek-official",
                UNPRICED,
            ),
            message(
                2,
                instant("2025-12-31T10:00:00Z").millis() + 1_000,
                r#""inputTokens":3000,"outputTokens":700"#,
            ),
        ])
        .as_bytes(),
    );

    write(
        "home-a/sessions/bad/no-header/session.v4.jsonl",
        document(&[message(
            1,
            instant("2025-12-31T10:00:00Z").millis(),
            never_counted,
        )])
        .as_bytes(),
    );
    write(
        "home-a/sessions/bad/corrupt/session.v4.jsonl.zstd",
        b"not a zstd frame",
    );
    write("home-a/sessions/bad/empty/session.v4.jsonl", b"");
    let stray = document(&[
        alpha_header,
        route(
            1,
            instant("2025-12-31T10:00:00Z").millis(),
            "deepseek-official",
            V4_PRO,
        ),
        message(1, instant("2025-12-31T10:00:00Z").millis(), never_counted),
    ]);
    for path in [
        "home-a/sessions/bad/notes/notes.jsonl",
        "home-a/sessions/bad/notes/session.vx.jsonl",
        "home-a/session.v4.jsonl",
    ] {
        write(path, stray.as_bytes());
    }

    let dsh = root.join("dsh");
    let mut padded = OsString::from(" ");
    padded.push(dsh.join("home-a"));
    let homes = vec![
        padded,
        " ".into(),
        dsh.join("home-b").into_os_string(),
        dsh.join("missing").into_os_string(),
        dsh.join("home-a").into_os_string(),
    ];
    vec![("DSH_HOME", homes)]
}

pub(crate) fn extra_cases(_root: &Path) -> Vec<Case> {
    let mut cases = vec![
        Case::new(
            "session",
            "json_debug",
            &["dsh", "session", "--offline", "--json", "--debug"],
        ),
        Case::new("daily", "table_warn", &["dsh", "daily", "--offline"]).env("LOG_LEVEL", "3"),
    ];
    cases.extend(
        ["daily", "weekly", "monthly", "session"].map(|kind| windowed_table(kind, "dsh", &[])),
    );
    cases
}
