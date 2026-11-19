use std::path::{Path, PathBuf};

use crate::golden::Case;

const V4_PRO: &str = "deepseek-v4-pro";
const FLASH: &str = "deepseek-flash";
const SONNET: &str = "anthropic/claude-sonnet-4.5";
const UNPRICED: &str = "dsh-mystery-1";

// Unix milliseconds on the shared timeline.
const DEC_27_2200: i64 = 1_766_872_800_000;
const DEC_30_0950: i64 = 1_767_088_200_000;
const DEC_30_1000: i64 = 1_767_088_800_000;
const DEC_31_1000: i64 = 1_767_175_200_000;
const DEC_31_1530: i64 = 1_767_195_000_000;
const DEC_31_2350: i64 = 1_767_225_000_000;
const JAN_01_0010: i64 = 1_767_226_200_000;
const JAN_01_1200: i64 = 1_767_268_800_000;
const JAN_01_1230: i64 = 1_767_270_600_000;
const JAN_02_0800: i64 = 1_767_340_800_000;
const JAN_02_1000: i64 = 1_767_348_000_000;

/// The first line of a session log. `fields` is the raw JSON after
/// `createdAt`.
fn header(created_at: i64, fields: &str) -> String {
    format!(r#"{{"type":"session","version":4,"createdAt":{created_at}{fields}}}"#)
}

/// One event line; `seq` and `time` are raw JSON so a case can omit or
/// mistype them.
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

/// An `assistant/message` settled with `usage`, the body of dsh's
/// `TokenUsage` object.
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

pub fn seed(root: &Path) -> Vec<(&'static str, PathBuf)> {
    let write = |relative: &str, contents: &[u8]| {
        let path = root.join("dsh").join(relative);
        std::fs::create_dir_all(path.parent().expect("fixture file has a parent"))
            .expect("failed to create DeepSeek Harness fixture directory");
        std::fs::write(&path, contents).expect("failed to write DeepSeek Harness fixture file");
    };
    let never_counted = r#""inputTokens":9999,"outputTokens":9999"#;

    let alpha_header = header(
        DEC_30_0950,
        r#","id":"sess-alpha","cwd":"/work/alpha","isSeeded":false,"delegationDepth":0"#,
    );
    let alpha_tokyo = message(
        5,
        DEC_31_1530,
        r#""inputTokens":400,"outputTokens":120,"cacheReadTokens":9000,"cacheWriteTokens":600,"totalTokens":10120"#,
    );
    let alpha = vec![
        alpha_header.clone(),
        route(1, DEC_30_1000, "deepseek-official", V4_PRO),
        event(
            "request/context",
            Some("2"),
            Some(&DEC_30_1000.to_string()),
            &format!(
                r#"{{"provider":"deepseek-official","model":"{V4_PRO}","contextWindow":1048576}}"#
            ),
        ),
        // The settled `usage` wins over the stream's usage chunks.
        event(
            "assistant/message",
            Some("3"),
            Some(&(DEC_30_1000 + 5_000).to_string()),
            r#"{"stream":[{"type":"chunk","chunk":{"type":"usage","usage":{"inputTokens":9999}}}],"usage":{"inputTokens":1200,"outputTokens":300,"cacheReadTokens":8000,"totalTokens":9500}}"#,
        ),
        // A failed stream settles as an attempt: its last usage chunk counts.
        event(
            "assistant/attempt",
            Some("4"),
            Some(&DEC_31_1000.to_string()),
            r#"{"stream":[{"type":"chunk","chunk":{"type":"usage","usage":{"inputTokens":5,"outputTokens":0}}},{"type":"chunk","chunk":{"type":"text","text":"x"}},{"type":"chunk","chunk":{"type":"usage","usage":{"inputTokens":50,"outputTokens":3}}}]}"#,
        ),
        // Already 2026-01-01 in Asia/Tokyo.
        alpha_tokyo.clone(),
        // The same `seq` again collapses into it; another `seq` is counted.
        alpha_tokyo,
        message(
            6,
            DEC_31_1530,
            r#""inputTokens":400,"outputTokens":120,"cacheReadTokens":9000,"cacheWriteTokens":600,"totalTokens":10120"#,
        ),
        "not json but mentions \"usage\"".to_string(),
        event(
            "request/context",
            Some("7"),
            Some(&JAN_01_0010.to_string()),
            &format!(r#"{{"provider":"openrouter","model":"{SONNET}","contextWindow":200000}}"#),
        ),
        message(
            8,
            JAN_01_0010,
            r#""inputTokens":300,"outputTokens":80,"cacheWriteTokens":200"#,
        ),
        // A route without a model leaves the current one in place.
        event(
            "request/context",
            Some("9"),
            Some(&JAN_01_0010.to_string()),
            r#"{"provider":"anthropic"}"#,
        ),
        // Only a total: it becomes output tokens.
        message(10, JAN_01_1200, r#""totalTokens":432"#),
        // A total above the parts: the remainder is kept as extra tokens.
        message(
            11,
            JAN_01_1200 + 60_000,
            r#""inputTokens":100,"outputTokens":50,"totalTokens":400"#,
        ),
        message(
            12,
            JAN_01_1200 + 120_000,
            r#""inputTokens":0,"outputTokens":0"#,
        ),
        // Summarisation names its own route; one without a model is `unknown`.
        event(
            "compaction/summary",
            Some("13"),
            Some(&JAN_01_1230.to_string()),
            &format!(
                r#"{{"compactionId":"c1","provider":"deepseek-account","model":"{FLASH}","usage":{{"inputTokens":19000,"outputTokens":800,"totalTokens":19800}}}}"#
            ),
        ),
        event(
            "compaction/summary",
            Some("14"),
            Some(&JAN_01_1230.to_string()),
            r#"{"provider":"deepseek-account","usage":{"inputTokens":90,"outputTokens":10}}"#,
        ),
        // Usage on any other event type is not a model call.
        event(
            "user/message",
            Some("15"),
            Some(&JAN_01_1230.to_string()),
            &format!("{{\"usage\":{{{never_counted}}}}}"),
        ),
        event(
            "assistant/message",
            Some("16"),
            Some(&JAN_01_1230.to_string()),
            "null",
        ),
        // A missing, zero, or string `time` falls back to `createdAt`.
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
        // Counts that are not non-negative integers read as zero.
        event(
            "assistant/message",
            Some("20"),
            Some(&JAN_01_1230.to_string()),
            r#"{"usage":{"inputTokens":"9999","outputTokens":7.5,"cacheReadTokens":-5,"cacheWriteTokens":40}}"#,
        ),
        r#"{"type":"assistant/message","seq":21,"data":{"usage":{"inputTokens":9999,"#.to_string(),
    ];
    write(
        "home-a/sessions/--work-alpha--/sess-alpha/session.v4.jsonl",
        document(&alpha).as_bytes(),
    );
    // An older generation dsh left behind after migrating the session.
    write(
        "home-a/sessions/--work-alpha--/sess-alpha/session.v3.jsonl",
        document(&[
            alpha_header.clone(),
            route(1, DEC_30_1000, "deepseek-official", V4_PRO),
            message(90, DEC_30_1000, never_counted),
        ])
        .as_bytes(),
    );
    write(
        "home-a/sessions/--work-alpha--/sess-alpha/session.lock",
        b"locked",
    );

    // A second DSH_HOME root holding a copy of the same session with one more
    // request: the shared `seq`s are read once.
    let mut alpha_copy = alpha[..alpha.len() - 1].to_vec();
    alpha_copy.push(message(
        30,
        JAN_02_0800,
        r#""inputTokens":800,"outputTokens":200,"cacheReadTokens":4000"#,
    ));
    write(
        "home-b/sessions/--work-alpha--/sess-alpha/session.v4.jsonl",
        document(&alpha_copy).as_bytes(),
    );

    // Compressed one zstd frame per write batch, ending in a torn frame whose
    // request is lost.
    let beta_head = header(DEC_31_2350, r#","id":"sess-beta","cwd":"/work/beta""#);
    let beta_parent_call = message(
        2,
        DEC_31_2350,
        r#""inputTokens":2000,"outputTokens":500,"cacheReadTokens":6000,"totalTokens":8500"#,
    );
    let beta_lines = [
        route(1, DEC_31_2350, "deepseek-official", V4_PRO),
        beta_parent_call.clone(),
        message(
            3,
            JAN_02_1000,
            r#""inputTokens":700,"outputTokens":90,"cacheWriteTokens":300"#,
        ),
    ];
    let mut beta = zstd::encode_all(format!("{beta_head}\n").as_bytes(), 0)
        .expect("failed to compress fixture");
    beta.extend(
        zstd::encode_all(format!("{}\n", beta_lines.join("\n")).as_bytes(), 0)
            .expect("failed to compress fixture"),
    );
    let torn = zstd::encode_all(message(4, JAN_02_1000, never_counted).as_bytes(), 0)
        .expect("failed to compress fixture");
    beta.extend(&torn[..torn.len() / 2]);
    write(
        "home-a/sessions/--work-beta--/sess-beta/session.v4.jsonl.zstd",
        &beta,
    );

    // A fork copies its parent's log up to the inherited marker; only what
    // follows the last such marker is its own usage.
    write(
        "home-a/sessions/--work-beta--/fork-beta/session.v4.jsonl",
        document(&[
            header(
                JAN_02_0800,
                r#","id":"fork-beta","cwd":"/work/beta","parentSession":"sess-beta","isSeeded":true"#,
            ),
            route(1, DEC_31_2350, "deepseek-official", V4_PRO),
            beta_parent_call.clone(),
            // Not inherited: the copied usage is still the parent's.
            event("session/end-seed", Some("3"), Some(&JAN_02_0800.to_string()), "{}"),
            event(
                "session/end-seed",
                Some("4"),
                Some(&JAN_02_0800.to_string()),
                r#"{"inherited":true}"#,
            ),
            // The inherited route still prices the fork's first call.
            message(5, JAN_02_0800 + 5_000, r#""inputTokens":600,"outputTokens":60"#),
            route(6, JAN_02_1000, "deepseek-account", FLASH),
            message(
                7,
                JAN_02_1000,
                r#""inputTokens":1500,"outputTokens":250,"cacheReadTokens":3000"#,
            ),
        ])
        .as_bytes(),
    );
    // A fork torn before its inherited marker holds only parent usage.
    write(
        "home-a/sessions/--work-beta--/fork-torn/session.v4.jsonl",
        document(&[
            header(JAN_02_0800, r#","id":"fork-torn","isSeeded":true"#),
            route(1, DEC_31_2350, "deepseek-official", V4_PRO),
            beta_parent_call,
        ])
        .as_bytes(),
    );

    // Generation 0 without an id, cwd, or route: the session is named after
    // its directory, the model is `unknown`, and `createdAt` dates it on the
    // stale day. Lines without `seq` dedupe on their content.
    let no_seq = event(
        "assistant/message",
        None,
        None,
        r#"{"usage":{"inputTokens":250,"outputTokens":40}}"#,
    );
    write(
        "home-a/sessions/_no-cwd/sess-legacy/session.jsonl",
        document(&[
            header(DEC_27_2200, ""),
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

    // Any depth under `sessions/` is a session directory.
    write(
        "home-a/sessions/deep/a/b/sess-deep/session.v2.jsonl",
        document(&[
            header(DEC_31_1000, r#","id":"sess-deep","cwd":"/work/deep""#),
            route(1, DEC_31_1000, "deepseek-official", UNPRICED),
            message(
                2,
                DEC_31_1000 + 1_000,
                r#""inputTokens":3000,"outputTokens":700"#,
            ),
        ])
        .as_bytes(),
    );

    // Not readable sessions: no header, not zstd, empty, or not a session file.
    write(
        "home-a/sessions/bad/no-header/session.v4.jsonl",
        document(&[message(1, DEC_31_1000, never_counted)]).as_bytes(),
    );
    write(
        "home-a/sessions/bad/corrupt/session.v4.jsonl.zstd",
        b"not a zstd frame",
    );
    write("home-a/sessions/bad/empty/session.v4.jsonl", b"");
    let stray = document(&[
        alpha_header,
        route(1, DEC_31_1000, "deepseek-official", V4_PRO),
        message(1, DEC_31_1000, never_counted),
    ]);
    for path in [
        "home-a/sessions/bad/notes/notes.jsonl",
        "home-a/sessions/bad/notes/session.vx.jsonl",
        "home-a/session.v4.jsonl",
    ] {
        write(path, stray.as_bytes());
    }

    // DSH_HOME is a comma-separated list: padded, with an empty item, a root
    // that does not exist, and a repeated root.
    let dsh = root.join("dsh");
    let homes = format!(
        " {a}, ,{b},{missing},{a}",
        a = dsh.join("home-a").display(),
        b = dsh.join("home-b").display(),
        missing = dsh.join("missing").display(),
    );
    vec![("DSH_HOME", PathBuf::from(homes))]
}

/// What the adapter says under `--debug` about the logs it cannot read.
pub fn extra_cases(_root: &Path) -> Vec<Case> {
    vec![Case::new(
        "session",
        "json_debug",
        &["dsh", "session", "--offline", "--json", "--debug"],
    )]
}
