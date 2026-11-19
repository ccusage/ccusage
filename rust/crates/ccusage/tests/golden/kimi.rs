use std::ffi::OsString;
use std::path::{Path, PathBuf};

use ccusage_test_support::{Instant, instant, set_modified};

pub(crate) fn seed(root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
    let legacy = root.join("kimi/legacy");
    let code = root.join("kimi/code");
    let write = |path: PathBuf, modified: Instant, lines: &[&str]| {
        std::fs::create_dir_all(path.parent().expect("fixture file has a parent"))
            .expect("failed to create Kimi fixture directory");
        std::fs::write(&path, lines.join("\n")).expect("failed to write Kimi fixture file");
        set_modified(&path, modified);
    };

    write(
        legacy.join("config.json"),
        instant("2025-12-28T00:00:00Z"),
        &[r#"{"model":"kimi-k2-thinking","default_thinking":true}"#],
    );
    write(
        code.join("config.json"),
        instant("2025-12-28T00:00:00Z"),
        &[r#"{"model":"gpt-5","#],
    );

    let a1 = &format!(
        r#"{{"timestamp":{timestamp},"message":{{"type":"StatusUpdate","payload":{{"token_usage":{{"input_other":1000,"output":200,"input_cache_read":4000,"input_cache_creation":300}},"message_id":"msg-a1"}}}}}}"#,
        timestamp = instant("2025-12-30T10:00:00.5Z").secs_f64()
    );
    let a2 = &format!(
        r#"{{"timestamp":{timestamp},"message":{{"type":"StatusUpdate","payload":{{"token_usage":{{"input_other":700,"output":150,"input_cache_read":2500,"input_cache_creation":0}},"message_id":"msg-a2"}}}}}}"#,
        timestamp = instant("2025-12-31T15:30:00.25Z").secs_f64()
    );
    write(
        legacy.join("sessions/group-a/session-old-1/wire.jsonl"),
        instant("2026-01-02T08:00:00Z"),
        &[
            r#"{"type":"metadata","protocol_version":"1.3","message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":9999,"output":9999},"message_id":"msg-metadata"}}}"#,
            &format!(
                r#"{{"timestamp":{timestamp:.1},"message":{{"type":"TurnBegin","payload":{{"user_input":"hello","token_usage":{{"input_other":9999,"output":9999}}}}}}}}"#,
                timestamp = instant("2025-12-30T09:58:20Z").secs_f64()
            ),
            a1,
            "",
            a2,
            a2,
            &format!(
                r#"{{"timestamp":{timestamp},"message":{{"type":"StatusUpdate","payload":{{"token_usage":{{"input_other":700,"output":150,"input_cache_read":2500,"input_cache_creation":0}},"message_id":"msg-a2"}}}}}}"#,
                timestamp = instant("2025-12-31T15:30:00.2504Z").secs_f64()
            ),
            &format!(
                r#"{{"timestamp":{timestamp},"message":{{"type":"StatusUpdate","payload":{{"token_usage":{{"input_other":700,"output":150,"input_cache_read":2500,"input_cache_creation":0}},"message_id":"msg-a2b"}}}}}}"#,
                timestamp = instant("2025-12-31T15:30:00.25Z").secs_f64()
            ),
            &format!(
                r#"{{"timestamp":{timestamp},"message":{{"type":"StatusUpdate","payload":{{"token_usage":{{"total":432}},"message_id":"msg-a3"}}}}}}"#,
                timestamp = instant("2026-01-01T12:00:00Z").secs()
            ),
            &format!(
                r#"{{"timestamp":{timestamp},"message":{{"type":"StatusUpdate","payload":{{"token_usage":{{"input_other":100,"output":50,"total":400}},"message_id":"msg-a4"}}}}}}"#,
                timestamp = instant("2026-01-01T12:01:00Z").secs()
            ),
            &format!(
                r#"{{"timestamp":{timestamp},"message":{{"type":"StatusUpdate","payload":{{"token_usage":{{"input_other":0,"output":0,"input_cache_read":0,"input_cache_creation":0}},"message_id":"msg-a5"}}}}}}"#,
                timestamp = instant("2026-01-01T12:02:00Z").secs()
            ),
            r#"{"timestamp":"2025-12-31T10:00:00Z","message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":80,"output":20},"message_id":"msg-a6"}}}"#,
            r#"{"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":60,"output":40}}}}"#,
            "not json but mentions \"token_usage\"",
            &format!(
                r#"{{"timestamp":{timestamp},"message":{{"type":"StatusUpdate","payload":{{"token_usage":{{"input_other":"9999","output":7.5,"input_cache_read":-5}},"message_id":"msg-a7"}}}}}}"#,
                timestamp = instant("2025-12-31T08:58:20Z").secs()
            ),
            &format!(
                r#"{{"timestamp":{timestamp},"message":{{"type":"StatusUpdate","payload":{{"token_usage":{{"input_other":"9999","output":30}},"message_id":"msg-a8"}}}}}}"#,
                timestamp = instant("2025-12-31T09:00:00Z").secs()
            ),
            &format!(
                r#"{{"timestamp":{timestamp},"message":{{"type":"StatusUpdate","payload":"token_usage 9999"}}}}"#,
                timestamp = instant("2025-12-31T09:01:40Z").secs()
            ),
            &format!(
                r#"{{"type":"wire.event","timestamp":{timestamp},"message":{{"type":"StatusUpdate","payload":{{"token_usage":{{"input_other":11,"output":9}},"message_id":"msg-a9"}}}}}}"#,
                timestamp = instant("2025-12-31T09:05:00Z").secs()
            ),
            &format!(
                r#"{{"timestamp":{timestamp},"message":{{"type":"StatusUpdate","payload":{{"token_usage":{{"input_other":9999,"outp"#,
                timestamp = instant("2025-12-31T09:06:40Z").secs()
            ),
        ],
    );

    write(
        legacy.join("sessions/group-b/session-old-1/wire.jsonl"),
        instant("2026-01-02T08:00:00Z"),
        &[
            a1,
            &format!(
                r#"{{"timestamp":{timestamp},"message":{{"type":"StatusUpdate","payload":{{"token_usage":{{"input_other":1000,"output":210,"input_cache_read":4000,"input_cache_creation":300}},"message_id":"msg-a1"}}}}}}"#,
                timestamp = instant("2025-12-30T10:00:00.5Z").secs_f64()
            ),
        ],
    );

    write(
        legacy.join("sessions/group-b/session-old-2/wire.jsonl"),
        instant("2025-12-28T00:00:00Z"),
        &[
            a1,
            &format!(
                r#"{{"timestamp":{timestamp},"message":{{"type":"StatusUpdate","payload":{{"token_usage":{{"input_other":100,"output":50}},"message_id":"msg-c1"}}}}}}"#,
                timestamp = instant("2025-12-27T22:00:00Z").secs()
            ),
            r#"{"timestamp":1e400,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":9999,"output":9999},"message_id":"msg-c2"}}}"#,
        ],
    );

    write(
        legacy.join("sessions/group-a/session-empty/wire.jsonl"),
        instant("2026-01-02T08:00:00Z"),
        &[],
    );

    write(
        legacy.join("sessions/odd/config.json"),
        instant("2025-12-28T00:00:00Z"),
        &[r#"{"model":"gpt-5"}"#],
    );
    write(
        legacy.join("sessions/odd/x/y/z/wire.jsonl"),
        instant("2026-01-02T08:00:00Z"),
        &[&format!(
            r#"{{"timestamp":{timestamp},"message":{{"type":"StatusUpdate","payload":{{"token_usage":{{"input_other":500,"output":100}},"message_id":"msg-z1"}}}}}}"#,
            timestamp = instant("2026-01-01T00:10:00Z").secs()
        )],
    );

    let not_a_wire_file: [&str; 1] = [&format!(
        r#"{{"timestamp":{timestamp},"message":{{"type":"StatusUpdate","payload":{{"token_usage":{{"input_other":5555,"output":5555}},"message_id":"msg-ignored"}}}}}}"#,
        timestamp = instant("2025-12-31T09:00:00Z").secs()
    )];
    for path in [
        "sessions/group-a/session-old-1/other.jsonl",
        "sessions/group-a/wire.jsonl",
        "sessions/deep/a/b/wire.jsonl",
        "wire.jsonl",
    ] {
        write(
            legacy.join(path),
            instant("2026-01-02T08:00:00Z"),
            &not_a_wire_file,
        );
    }

    let n1 = &format!(
        r#"{{"type":"usage.record","model":"kimi-code/kimi-for-coding","usage":{{"inputOther":3000,"output":80,"inputCacheRead":14000,"inputCacheCreation":0}},"usageScope":"turn","time":{time}}}"#,
        time = instant("2025-12-30T23:30:00Z").millis()
    );
    let n2 = &format!(
        r#"{{"type":"usage.record","model":"claude-sonnet-4-20250514","usage":{{"inputOther":900,"output":400,"inputCacheRead":7000,"inputCacheCreation":200}},"usageScope":"turn","time":{time},"costUSD":99.0}}"#,
        time = instant("2025-12-31T23:50:00Z").millis()
    );
    write(
        code.join("sessions/ws-1/session-new-1/agents/main/wire.jsonl"),
        instant("2026-01-01T06:00:00Z"),
        &[
            r#"{"type":"metadata","protocol_version":"2.0"}"#,
            n1,
            &format!(
                r#"{{"type":"usage.record","model":"kimi-code/kimi-for-coding","usage":{{"inputOther":9999,"output":9999,"inputCacheRead":9999,"inputCacheCreation":9999}},"usageScope":"session","time":{time}}}"#,
                time = instant("2025-12-30T23:30:00.5Z").millis()
            ),
            n2,
            n2,
            &format!(
                r#"{{"type":"usage.record","model":"claude-sonnet-4-20250514","usage":{{"inputOther":900,"output":400,"inputCacheRead":7000,"inputCacheCreation":200}},"usageScope":"turn","time":{time},"costUSD":99.0}}"#,
                time = instant("2025-12-31T23:50:00.001Z").millis()
            ),
            &format!(
                r#"{{"type":"usage.record","model":"kimi-mystery-1","usage":{{"inputOther":300,"output":70}},"usageScope":"turn","time":{time}}}"#,
                time = instant("2026-01-01T12:00:00Z").millis()
            ),
            &format!(
                r#"{{"type":"usage.record","usage":{{"inputOther":200,"output":45}},"usageScope":"turn","time":{time}}}"#,
                time = instant("2026-01-01T12:30:00Z").millis()
            ),
            r#"{"type":"usage.record","model":"gpt-5","usage":{"inputOther":600,"output":100,"inputCacheRead":3000},"usageScope":"turn"}"#,
            &format!(
                r#"{{"type":"usage.record","model":"kimi-code/kimi-k2.5","usage":{{"inputOther":120,"output":30}},"usageScope":"turn","time":{time}}}"#,
                time = instant("2025-12-31T00:00:00.0005Z").millis_f64()
            ),
            &format!(
                r#"{{"type":"usage.record","model":"gpt-5","usage":{{"inputOther":9999,"output":9999}},"time":{time}}}"#,
                time = instant("2026-01-01T12:01:40Z").millis()
            ),
            &format!(
                r#"{{"type":"usage.record","model":"gpt-5","usage":{{"inputOther":7777,"output":7777}},"usageScope":"Turn","time":{time}}}"#,
                time = instant("2026-01-01T12:01:40Z").millis()
            ),
            &format!(
                r#"{{"type":"usage.record","model":"gpt-5","usageScope":"turn","time":{time}}}"#,
                time = instant("2026-01-01T12:01:40Z").millis()
            ),
            &format!(
                r#"{{"type":"usage.record","model":"gpt-5","usage":"9999 tokens","usageScope":"turn","time":{time}}}"#,
                time = instant("2026-01-01T12:01:40Z").millis()
            ),
            &format!(
                r#"{{"type":"usage.record","model":"gpt-5","usage":{{"inputOther":0,"output":0}},"usageScope":"turn","time":{time}}}"#,
                time = instant("2026-01-01T12:01:40Z").millis()
            ),
            &format!(
                r#"{{"type":"usage.record","model":"gpt-5","message":"turn finished","usage":{{"inputOther":6666,"output":6666}},"usageScope":"turn","time":{time}}}"#,
                time = instant("2026-01-01T12:01:40Z").millis()
            ),
            &format!(
                r#"{{"type":"turn.begin","note":"usage.record follows","usage":{{"inputOther":9999,"output":9999}},"usageScope":"turn","time":{time}}}"#,
                time = instant("2026-01-01T12:01:40Z").millis()
            ),
            r#"{"type":"usage.record","model":"gpt-5","usage":{"inputOther":9999,"#,
        ],
    );

    write(
        code.join("sessions/ws-1/session-new-1/agents/sub-1/wire.jsonl"),
        instant("2026-01-01T06:00:00Z"),
        &[
            n1,
            &format!(
                r#"{{"type":"usage.record","model":"kimi-code/kimi-for-coding","usage":{{"inputOther":800,"output":300,"inputCacheRead":6000,"inputCacheCreation":50}},"usageScope":"turn","time":{time}}}"#,
                time = instant("2026-01-02T10:00:00Z").millis()
            ),
            &format!(
                r#"{{"type":"usage.record","model":"kimi-code/kimi-for-coding","usage":{{"inputOther":1000,"output":500,"inputCacheRead":2000}},"usageScope":"turn","time":{time}}}"#,
                time = instant("2026-04-21T00:00:00Z").millis()
            ),
        ],
    );

    write(
        code.join("sessions/ws-2/session-new-2/agents/main/wire.jsonl"),
        instant("2026-01-02T08:00:00Z"),
        &[
            n2,
            &format!(
                r#"{{"type":"usage.record","model":"kimi-code/kimi-for-coding","usage":{{"inputOther":1000,"output":500,"inputCacheRead":2000}},"usageScope":"turn","time":{time}}}"#,
                time = instant("2025-12-31T16:00:00Z").millis()
            ),
        ],
    );

    write(
        code.join("sessions/group-c/session-old-3/wire.jsonl"),
        instant("2026-01-02T08:00:00Z"),
        &[
            &format!(
                r#"{{"timestamp":{timestamp},"message":{{"type":"StatusUpdate","payload":{{"token_usage":{{"input_other":400,"output":100,"input_cache_read":1000,"input_cache_creation":20}},"message_id":"msg-o1"}}}}}}"#,
                timestamp = instant("2025-12-31T14:59:59.9995Z").secs_f64()
            ),
            &format!(
                r#"{{"timestamp":{timestamp},"message":{{"type":"StatusUpdate","payload":{{"token_usage":{{"input_other":250,"output":50}},"message_id":"msg-o2"}}}}}}"#,
                timestamp = instant("2025-12-31T15:00:00Z").secs()
            ),
        ],
    );

    write(
        code.join("sessions/ws-1/session-new-1/agents/empty/wire.jsonl"),
        instant("2026-01-01T06:00:00Z"),
        &[],
    );
    write(
        code.join("sessions/ws-1/session-new-1/agents/main/context.jsonl"),
        instant("2026-01-01T06:00:00Z"),
        &[&format!(
            r#"{{"type":"usage.record","model":"gpt-5","usage":{{"inputOther":5555,"output":5555}},"usageScope":"turn","time":{time}}}"#,
            time = instant("2026-01-01T12:01:40Z").millis()
        )],
    );

    let mut padded = OsString::from(" ");
    padded.push(code);
    let data_dirs = vec![
        legacy.clone().into_os_string(),
        padded,
        OsString::new(),
        root.join("kimi/missing").into_os_string(),
        legacy.into_os_string(),
    ];
    vec![("KIMI_DATA_DIR", data_dirs)]
}
