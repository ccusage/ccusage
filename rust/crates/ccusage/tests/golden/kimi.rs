use std::path::{Path, PathBuf};

// File mtimes double as the timestamp of every record that carries none, so
// each one is a deliberate point on the shared timeline.
const MTIME_2025_12_28: u64 = 1_766_880_000;
const MTIME_2026_01_01_0600: u64 = 1_767_247_200;
const MTIME_2026_01_02_0800: u64 = 1_767_340_800;

pub fn seed(root: &Path) -> Vec<(&'static str, PathBuf)> {
    let legacy = root.join("kimi/legacy");
    let code = root.join("kimi/code");
    let write = |path: PathBuf, modified: u64, lines: &[&str]| {
        std::fs::create_dir_all(path.parent().expect("fixture file has a parent"))
            .expect("failed to create Kimi fixture directory");
        std::fs::write(&path, lines.join("\n")).expect("failed to write Kimi fixture file");
        crate::golden::set_modified(&path, modified);
    };

    // The legacy root names its model in config.json; old-format lines carry none.
    write(
        legacy.join("config.json"),
        MTIME_2025_12_28,
        &[r#"{"model":"kimi-k2-thinking","default_thinking":true}"#],
    );
    // The code root's config is unreadable, so its old-format lines fall back
    // to `kimi-for-coding`.
    write(
        code.join("config.json"),
        MTIME_2025_12_28,
        &[r#"{"model":"gpt-5","#],
    );

    let a1 = r#"{"timestamp":1767088800.5,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":1000,"output":200,"input_cache_read":4000,"input_cache_creation":300},"message_id":"msg-a1"}}}"#;
    let a2 = r#"{"timestamp":1767195000.25,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":700,"output":150,"input_cache_read":2500,"input_cache_creation":0},"message_id":"msg-a2"}}}"#;
    write(
        legacy.join("sessions/group-a/session-old-1/wire.jsonl"),
        MTIME_2026_01_02_0800,
        &[
            r#"{"type":"metadata","protocol_version":"1.3","message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":9999,"output":9999},"message_id":"msg-metadata"}}}"#,
            r#"{"timestamp":1767088700.0,"message":{"type":"TurnBegin","payload":{"user_input":"hello","token_usage":{"input_other":9999,"output":9999}}}}"#,
            a1,
            "",
            a2,
            // Replayed verbatim, then again with a timestamp that differs below
            // the millisecond the dedupe key keeps: both collapse into `a2`.
            a2,
            r#"{"timestamp":1767195000.2504,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":700,"output":150,"input_cache_read":2500,"input_cache_creation":0},"message_id":"msg-a2"}}}"#,
            // Near miss: only the message id differs, so it is counted.
            r#"{"timestamp":1767195000.25,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":700,"output":150,"input_cache_read":2500,"input_cache_creation":0},"message_id":"msg-a2b"}}}"#,
            // Only a total: it becomes output tokens.
            r#"{"timestamp":1767268800,"message":{"type":"StatusUpdate","payload":{"token_usage":{"total":432},"message_id":"msg-a3"}}}"#,
            // A total above the parts: the remainder is kept as extra tokens.
            r#"{"timestamp":1767268860,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":100,"output":50,"total":400},"message_id":"msg-a4"}}}"#,
            r#"{"timestamp":1767268920,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":0,"output":0,"input_cache_read":0,"input_cache_creation":0},"message_id":"msg-a5"}}}"#,
            // A string timestamp and a missing one both fall back to the mtime.
            r#"{"timestamp":"2025-12-31T10:00:00Z","message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":80,"output":20},"message_id":"msg-a6"}}}"#,
            r#"{"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":60,"output":40}}}}"#,
            "not json but mentions \"token_usage\"",
            // Counts that are not non-negative integers read as zero.
            r#"{"timestamp":1767171500,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":"9999","output":7.5,"input_cache_read":-5},"message_id":"msg-a7"}}}"#,
            r#"{"timestamp":1767171600,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":"9999","output":30},"message_id":"msg-a8"}}}"#,
            r#"{"timestamp":1767171700,"message":{"type":"StatusUpdate","payload":"token_usage 9999"}}"#,
            // An unknown top-level type still takes the old-format path.
            r#"{"type":"wire.event","timestamp":1767171900,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":11,"output":9},"message_id":"msg-a9"}}}"#,
            r#"{"timestamp":1767172000,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":9999,"outp"#,
        ],
    );

    // The same session directory name under another group: `a1` replayed
    // across files is dropped, the copy with one more output token is not.
    write(
        legacy.join("sessions/group-b/session-old-1/wire.jsonl"),
        MTIME_2026_01_02_0800,
        &[
            a1,
            r#"{"timestamp":1767088800.5,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":1000,"output":210,"input_cache_read":4000,"input_cache_creation":300},"message_id":"msg-a1"}}}"#,
        ],
    );

    // Another session: the same `a1` line is counted again.
    write(
        legacy.join("sessions/group-b/session-old-2/wire.jsonl"),
        MTIME_2025_12_28,
        &[
            a1,
            r#"{"timestamp":1766872800,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":100,"output":50},"message_id":"msg-c1"}}}"#,
            r#"{"timestamp":1e400,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":9999,"output":9999},"message_id":"msg-c2"}}}"#,
        ],
    );

    write(
        legacy.join("sessions/group-a/session-empty/wire.jsonl"),
        MTIME_2026_01_02_0800,
        &[],
    );

    // Five path components without an `agents` directory: accepted as a wire
    // file, with the session named after the innermost directory and the
    // config looked up four levels above the file.
    write(
        legacy.join("sessions/odd/config.json"),
        MTIME_2025_12_28,
        &[r#"{"model":"gpt-5"}"#],
    );
    write(
        legacy.join("sessions/odd/x/y/z/wire.jsonl"),
        MTIME_2026_01_02_0800,
        &[
            r#"{"timestamp":1767226200,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":500,"output":100},"message_id":"msg-z1"}}}"#,
        ],
    );

    // Not wire files: wrong name, wrong depth, or outside `sessions`.
    let not_a_wire_file = [
        r#"{"timestamp":1767171600,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":5555,"output":5555},"message_id":"msg-ignored"}}}"#,
    ];
    for path in [
        "sessions/group-a/session-old-1/other.jsonl",
        "sessions/group-a/wire.jsonl",
        "sessions/deep/a/b/wire.jsonl",
        "wire.jsonl",
    ] {
        write(legacy.join(path), MTIME_2026_01_02_0800, &not_a_wire_file);
    }

    let n1 = r#"{"type":"usage.record","model":"kimi-code/kimi-for-coding","usage":{"inputOther":3000,"output":80,"inputCacheRead":14000,"inputCacheCreation":0},"usageScope":"turn","time":1767137400000}"#;
    let n2 = r#"{"type":"usage.record","model":"claude-sonnet-4-20250514","usage":{"inputOther":900,"output":400,"inputCacheRead":7000,"inputCacheCreation":200},"usageScope":"turn","time":1767225000000,"costUSD":99.0}"#;
    write(
        code.join("sessions/ws-1/session-new-1/agents/main/wire.jsonl"),
        MTIME_2026_01_01_0600,
        &[
            r#"{"type":"metadata","protocol_version":"2.0"}"#,
            n1,
            // Session-scoped records are cumulative totals of the turn records.
            r#"{"type":"usage.record","model":"kimi-code/kimi-for-coding","usage":{"inputOther":9999,"output":9999,"inputCacheRead":9999,"inputCacheCreation":9999},"usageScope":"session","time":1767137400500}"#,
            n2,
            n2,
            // Near miss: one millisecond later, so it is counted.
            r#"{"type":"usage.record","model":"claude-sonnet-4-20250514","usage":{"inputOther":900,"output":400,"inputCacheRead":7000,"inputCacheCreation":200},"usageScope":"turn","time":1767225000001,"costUSD":99.0}"#,
            r#"{"type":"usage.record","model":"kimi-mystery-1","usage":{"inputOther":300,"output":70},"usageScope":"turn","time":1767268800000}"#,
            r#"{"type":"usage.record","usage":{"inputOther":200,"output":45},"usageScope":"turn","time":1767270600000}"#,
            // A missing `time` and a fractional one both fall back to the mtime.
            r#"{"type":"usage.record","model":"gpt-5","usage":{"inputOther":600,"output":100,"inputCacheRead":3000},"usageScope":"turn"}"#,
            r#"{"type":"usage.record","model":"kimi-code/kimi-k2.5","usage":{"inputOther":120,"output":30},"usageScope":"turn","time":1767139200000.5}"#,
            r#"{"type":"usage.record","model":"gpt-5","usage":{"inputOther":9999,"output":9999},"time":1767268900000}"#,
            r#"{"type":"usage.record","model":"gpt-5","usage":{"inputOther":7777,"output":7777},"usageScope":"Turn","time":1767268900000}"#,
            r#"{"type":"usage.record","model":"gpt-5","usageScope":"turn","time":1767268900000}"#,
            r#"{"type":"usage.record","model":"gpt-5","usage":"9999 tokens","usageScope":"turn","time":1767268900000}"#,
            r#"{"type":"usage.record","model":"gpt-5","usage":{"inputOther":0,"output":0},"usageScope":"turn","time":1767268900000}"#,
            // A turn record that also has a non-object `message` fails to parse
            // as a whole.
            r#"{"type":"usage.record","model":"gpt-5","message":"turn finished","usage":{"inputOther":6666,"output":6666},"usageScope":"turn","time":1767268900000}"#,
            r#"{"type":"turn.begin","note":"usage.record follows","usage":{"inputOther":9999,"output":9999},"usageScope":"turn","time":1767268900000}"#,
            r#"{"type":"usage.record","model":"gpt-5","usage":{"inputOther":9999,"#,
        ],
    );

    // A second agent of the same session. Its copy of `n1` is dropped because
    // the dedupe key has no agent in it.
    write(
        code.join("sessions/ws-1/session-new-1/agents/sub-1/wire.jsonl"),
        MTIME_2026_01_01_0600,
        &[
            n1,
            r#"{"type":"usage.record","model":"kimi-code/kimi-for-coding","usage":{"inputOther":800,"output":300,"inputCacheRead":6000,"inputCacheCreation":50},"usageScope":"turn","time":1767348000000}"#,
            // After the cutoff at which `kimi-for-coding` is priced as K2.6
            // instead of K2.5; the same token counts appear before it in
            // session-new-2.
            r#"{"type":"usage.record","model":"kimi-code/kimi-for-coding","usage":{"inputOther":1000,"output":500,"inputCacheRead":2000},"usageScope":"turn","time":1776729600000}"#,
        ],
    );

    // Another session: the same `n2` line is counted again.
    write(
        code.join("sessions/ws-2/session-new-2/agents/main/wire.jsonl"),
        MTIME_2026_01_02_0800,
        &[
            n2,
            r#"{"type":"usage.record","model":"kimi-code/kimi-for-coding","usage":{"inputOther":1000,"output":500,"inputCacheRead":2000},"usageScope":"turn","time":1767196800000}"#,
        ],
    );

    // Old layout and format beside the new one; one millisecond apart across
    // the Asia/Tokyo midnight.
    write(
        code.join("sessions/group-c/session-old-3/wire.jsonl"),
        MTIME_2026_01_02_0800,
        &[
            r#"{"timestamp":1767193199.9995,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":400,"output":100,"input_cache_read":1000,"input_cache_creation":20},"message_id":"msg-o1"}}}"#,
            r#"{"timestamp":1767193200,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":250,"output":50},"message_id":"msg-o2"}}}"#,
        ],
    );

    write(
        code.join("sessions/ws-1/session-new-1/agents/empty/wire.jsonl"),
        MTIME_2026_01_01_0600,
        &[],
    );
    write(
        code.join("sessions/ws-1/session-new-1/agents/main/context.jsonl"),
        MTIME_2026_01_01_0600,
        &[
            r#"{"type":"usage.record","model":"gpt-5","usage":{"inputOther":5555,"output":5555},"usageScope":"turn","time":1767268900000}"#,
        ],
    );

    // KIMI_DATA_DIR is a comma-separated list: padded, with an empty item, a
    // directory that does not exist, and a repeated root.
    let data_dirs = format!(
        "{legacy}, {code},,{missing},{legacy}",
        legacy = legacy.display(),
        code = code.display(),
        missing = root.join("kimi/missing").display(),
    );
    vec![("KIMI_DATA_DIR", PathBuf::from(data_dirs))]
}
