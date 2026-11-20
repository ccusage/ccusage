use std::ffi::OsString;
use std::path::{Path, PathBuf};

use ccusage_test_support::{Instant, instant, set_modified};

const CODER: &str = "qwen3-coder-plus";
const SONNET: &str = "claude-sonnet-4-20250514";
const UNPRICED: &str = "qwen-mystery-1";
const QWEN_PREFIXED: &str = "qwen-2.5-72b-instruct";
const ALIBABA_PREFIXED: &str = "qwen-3-30b";

fn assistant(session: &str, timestamp: &str, model: &str, usage: &str) -> String {
    format!(
        r#"{{"type":"assistant","sessionId":"{session}","timestamp":"{timestamp}","model":"{model}","usageMetadata":{{{usage}}}}}"#
    )
}

pub(crate) fn seed(root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
    let store = root.join("qwen");
    let primary = store.join("primary");
    let secondary = store.join("secondary");
    let write = |path: PathBuf, contents: &str, mtime: Instant| {
        std::fs::create_dir_all(path.parent().expect("fixture file has a parent"))
            .expect("failed to create Qwen fixture directory");
        std::fs::write(&path, contents).expect("failed to write Qwen fixture file");
        set_modified(&path, mtime);
    };

    let a1_usage = r#""promptTokenCount":1000,"candidatesTokenCount":200,"thoughtsTokenCount":50,"cachedContentTokenCount":400,"totalTokenCount":1250"#;
    let a1 = assistant("sess-a", "2025-12-30T09:00:00.000Z", CODER, a1_usage);
    let a2_usage =
        r#""promptTokenCount":500,"candidatesTokenCount":120,"cachedContentTokenCount":80"#;
    let a2 = assistant("sess-a", "2025-12-31T15:30:00.000Z", SONNET, a2_usage);
    let a4_usage = r#""promptTokenCount":100,"candidatesTokenCount":40,"totalTokenCount":200"#;

    write(
        primary.join("projects/alpha/chats/session-a.jsonl"),
        &[
            r#"{"type":"user","sessionId":"sess-a","timestamp":"2025-12-30T08:59:00.000Z","model":"qwen3-coder-plus","usageMetadata":{"promptTokenCount":9999,"candidatesTokenCount":9999}}"#.to_string(),
            a1.clone(),
            a1.clone(),
            assistant("sess-a", "2025-12-30T09:00:00Z", CODER, a1_usage),
            assistant(
                "sess-a",
                "2025-12-30T09:00:00.000Z",
                CODER,
                r#""promptTokenCount":1000,"candidatesTokenCount":201,"thoughtsTokenCount":50,"cachedContentTokenCount":400,"totalTokenCount":1251"#,
            ),
            a2.clone(),
            assistant("sess-a", "2025-12-31T23:59:59.999Z", UNPRICED, r#""totalTokenCount":654"#),
            assistant("sess-a", "2026-01-01T00:30:00.000Z", CODER, a4_usage),
            assistant(" sess-a ", "2026-01-01T00:30:00.000Z", " qwen3-coder-plus ", a4_usage),
            assistant(
                "sess-a",
                "2026-01-01T08:59:59+09:00",
                QWEN_PREFIXED,
                r#""promptTokenCount":300,"candidatesTokenCount":60"#,
            ),
            assistant(
                "sess-a",
                "2026-01-01T10:00:00.000Z",
                CODER,
                r#""promptTokenCount":12.9,"candidatesTokenCount":"77","cachedContentTokenCount":-5,"thoughtsTokenCount":7"#,
            ),
            assistant(
                "sess-a",
                "2026-01-01T10:30:00.000Z",
                "qwen-zero-9",
                r#""promptTokenCount":0,"candidatesTokenCount":0,"totalTokenCount":0"#,
            ),
            r#"{"type":"assistant","sessionId":"sess-a","timestamp":"2026-01-01T10:31:00.000Z","model":"qwen-null-usage","usageMetadata":null}"#.to_string(),
            r#"{"type":"assistant","sessionId":"sess-a","timestamp":"2026-01-01T10:32:00.000Z","model":"qwen-string-usage","usageMetadata":"9999"}"#.to_string(),
            r#"{"type":"assistant","sessionId":"sess-a","timestamp":"2026-01-01T10:33:00.000Z","model":"qwen-nested-usage","message":{"usageMetadata":{"promptTokenCount":6666,"candidatesTokenCount":6666}}}"#.to_string(),
            r#"{"type":" assistant ","sessionId":"sess-a","timestamp":"2026-01-01T11:00:00.000Z","model":"qwen3-coder-plus","usageMetadata":{"promptTokenCount":11,"candidatesTokenCount":3}}"#.to_string(),
            r#"{"type":"Assistant","sessionId":"sess-a","timestamp":"2026-01-01T11:01:00.000Z","model":"qwen3-coder-plus","usageMetadata":{"promptTokenCount":7777,"candidatesTokenCount":7777}}"#.to_string(),
            r#"{"type":"system","subtype":"ui_telemetry","sessionId":"sess-a","timestamp":"2026-01-01T11:02:00.000Z","model":"qwen3-coder-plus","usageMetadata":{"promptTokenCount":8888,"candidatesTokenCount":8888}}"#.to_string(),
            r#"{"type":"assistant","sessionId":"sess-a","timestamp":"2026-01-01T11:03:00.000Z","model":"qwen3-coder-plus","usageMetadata":{"promptTokenCount":9999,"candi"#.to_string(),
            String::new(),
            r#"{"type":"assistant","timestamp":"2026-01-02T08:00:00.000Z","usageMetadata":{"promptTokenCount":70,"candidatesTokenCount":30}}"#.to_string(),
            assistant("sess-a", "yesterday", CODER, r#""promptTokenCount":60,"candidatesTokenCount":15"#),
            r#"{"type":"assistant","sessionId":"sess-a","model":"qwen3-coder-plus","usageMetadata":{"promptTokenCount":61,"candidatesTokenCount":16}}"#.to_string(),
            format!(r#"{{"type":"assistant","sessionId":"sess-a","timestamp":{timestamp},"model":"qwen3-coder-plus","usageMetadata":{{"promptTokenCount":61,"candidatesTokenCount":16}}}}"#, timestamp = instant("2026-01-01T00:00:00Z").millis()),
        ]
        .join("\n"),
        instant("2026-01-02T12:00:00Z"),
    );

    write(
        primary.join("projects/beta/chats/session-b.jsonl"),
        &[
            assistant(
                "sess-b",
                "2025-12-27T22:00:00.000Z",
                SONNET,
                r#""promptTokenCount":100,"candidatesTokenCount":50"#,
            ),
            assistant(
                "sess-b",
                "2025-12-30T20:00:00.000Z",
                UNPRICED,
                r#""promptTokenCount":33,"candidatesTokenCount":11,"cachedContentTokenCount":500"#,
            ),
            assistant(
                "sess-b",
                "2025-12-31T14:59:59.000Z",
                CODER,
                r#""promptTokenCount":2000,"candidatesTokenCount":300,"thoughtsTokenCount":100,"cachedContentTokenCount":1500,"totalTokenCount":2400"#,
            ),
            assistant(
                "sess-b",
                "2025-12-31T15:00:00.000Z",
                ALIBABA_PREFIXED,
                r#""promptTokenCount":310,"candidatesTokenCount":65"#,
            ),
            a2,
            assistant("sess-b", "2025-12-31T15:30:00.000Z", SONNET, a2_usage),
            assistant(
                "sess-b",
                "2026-01-01T12:00:00.000Z",
                CODER,
                r#""promptTokenCount":300000,"candidatesTokenCount":2000,"cachedContentTokenCount":100000"#,
            ),
            assistant(
                "sess-b",
                "2026-01-02T23:30:00.000Z",
                SONNET,
                r#""promptTokenCount":500,"candidatesTokenCount":80,"cachedContentTokenCount":2000"#,
            ),
        ]
        .join("\n"),
        instant("2026-01-02T23:30:00Z"),
    );

    let fallback = format!(
        r#"{{"type":"assistant","model":"{CODER}","usageMetadata":{{"promptTokenCount":150,"candidatesTokenCount":25}}}}"#
    );
    write(
        primary.join("projects/alpha/chats/fallback-c.jsonl"),
        &fallback,
        instant("2025-12-31T16:00:00Z"),
    );
    write(
        primary.join("projects/beta/chats/fallback-c.jsonl"),
        &fallback,
        instant("2025-12-31T16:00:00Z"),
    );

    write(
        primary.join("projects/beta/chats/archive/fallback-c.jsonl"),
        &fallback,
        instant("2025-12-31T16:00:00Z"),
    );
    write(
        primary.join("projects/alpha/chats/archive/session-e.jsonl"),
        &assistant(
            "sess-e",
            "2026-01-01T10:00:00.000Z",
            CODER,
            r#""promptTokenCount":420,"candidatesTokenCount":70,"cachedContentTokenCount":100"#,
        ),
        instant("2026-01-01T09:00:00Z"),
    );

    write(
        secondary.join("projects/alpha/chats/session-a.jsonl"),
        &[
            a1,
            assistant(
                "sess-d",
                "2026-01-01T09:00:00.000Z",
                SONNET,
                r#""promptTokenCount":250,"candidatesTokenCount":50"#,
            ),
            String::new(),
        ]
        .join("\r\n"),
        instant("2026-01-01T09:00:00Z"),
    );

    let ignored = assistant(
        "sess-ignored",
        "2025-12-31T12:00:00.000Z",
        CODER,
        r#""promptTokenCount":9999,"candidatesTokenCount":9999"#,
    );
    for relative in [
        "chats/outside-projects.jsonl",
        "tmp/alpha/chats/wrong-root.jsonl",
        "projects/loose.jsonl",
        "projects/alpha/loose.jsonl",
        "projects/alpha/logs/wrong-directory.jsonl",
        "projects/alpha/chats/nested/too-deep.jsonl",
        "projects/alpha/chats/archive/nested/too-deep.jsonl",
        "projects/alpha/archive/wrong-directory.jsonl",
        "projects/alpha/chats/wrong-extension.json",
        "projects/alpha/chats/wrong-extension.jsonl.bak",
    ] {
        write(
            primary.join(relative),
            &ignored,
            instant("2025-12-27T22:00:00Z"),
        );
    }
    write(
        primary.join("projects/alpha/chats/empty.jsonl"),
        "",
        instant("2025-12-27T22:00:00Z"),
    );
    write(
        primary.join("projects/beta/chats/truncated.jsonl"),
        r#"{"type":"assistant","sessionId":"sess-truncated","timestamp":"2025-12-31T12:00:00.000Z","model":"qwen3-coder-plus","usageMetadata":{"promptTokenCount":9999,"candi"#,
        instant("2025-12-27T22:00:00Z"),
    );

    let roots = [
        primary.clone(),
        secondary,
        primary,
        store.join("missing-root"),
    ]
    .map(PathBuf::into_os_string)
    .to_vec();
    vec![("QWEN_DATA_DIR", roots)]
}
