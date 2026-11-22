use std::ffi::OsString;
use std::path::{Path, PathBuf};

use ccusage_test_support::{instant, set_modified};

fn write_chat(data_root: &Path, project: &str, chat_id: &str, contents: &str) -> PathBuf {
    let dir = data_root
        .join("projects")
        .join(project)
        .join("chats")
        .join(chat_id);
    std::fs::create_dir_all(&dir).expect("failed to create Codebuff chat directory");
    let path = dir.join("chat-messages.json");
    std::fs::write(&path, contents).expect("failed to write Codebuff chat file");
    path
}

pub(crate) fn seed(root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
    let manicode = root.join("codebuff/manicode");
    let backup = root.join("codebuff/backup/manicode");
    let dev = root.join("codebuff/manicode-dev");

    let ledger_chat = write_chat(
        &manicode,
        "alpha",
        "2025-12-30T10-00-00.000Z",
        &format!(
            r#"[
  {{"id":"u1","role":"user","timestamp":"2025-12-30T10:00:00.000Z","metadata":{{"model":"claude-sonnet-4-20250514","usage":{{"inputTokens":9999,"outputTokens":9999}}}},"credits":9.0}},
  "garbage",
  {{"id":"a1","role":"assistant","timestamp":"2025-12-30T10:00:01.000Z","metadata":{{"model":"claude-sonnet-4-20250514","usage":{{"inputTokens":1000,"outputTokens":200,"cacheCreationInputTokens":300,"cacheReadInputTokens":4000}}}},"credits":1.5}},
  {{"id":"a2","variant":"ai","timestamp":"2025-12-31T15:30:00.000Z","metadata":{{"model":"gpt-5","usage":{{"inputTokens":9999,"outputTokens":9999}}}},"credits":9.0}},
  {{"id":"a2","variant":"ai","timestamp":"2025-12-31T15:30:00.000Z","metadata":{{"usage":{{"model":"gpt-5","input_tokens":700,"output_tokens":150,"cache_read_input_tokens":2500,"credits":0.75}}}}}},
  {{"id":"a3","variant":"agent","createdAt":{created_at},"metadata":{{"codebuff":{{"usage":{{"model":"codebuff-mystery-1","promptTokens":300,"completionTokens":70}}}}}},"credits":0.2}},
  {{"id":"a4","role":"assistant","metadata":{{"timestamp":{timestamp},"model":"gpt-5","usage":{{"totalTokens":345}}}}}},
  {{"id":"a5","role":"assistant","timestamp":"2026-01-01T13:00:00.000Z","metadata":{{"model":"claude-sonnet-4-20250514","usage":{{"inputTokens":0,"outputTokens":0}}}}}},
  {{"id":"a6","variant":"user","role":"assistant","timestamp":"2026-01-01T13:10:00.000Z","metadata":{{"model":"claude-sonnet-4-20250514","usage":{{"inputTokens":8888,"outputTokens":8888}}}}}},
  {{"id":"a7","variant":"tool","timestamp":"2026-01-01T13:20:00.000Z","metadata":{{"model":"claude-sonnet-4-20250514","usage":{{"inputTokens":7777,"outputTokens":7777}}}},"credits":7.0}}
]"#,
            created_at = instant("2026-01-01T12:00:00Z").secs(),
            timestamp = instant("2026-01-01T12:30:00Z").millis()
        ),
    );
    let chat_dir = ledger_chat.parent().expect("chat file has a parent");
    let ignored_usage = r#"[{"id":"x1","role":"assistant","timestamp":"2025-12-31T12:00:00.000Z","metadata":{"model":"claude-sonnet-4-20250514","usage":{"inputTokens":5555,"outputTokens":5555}}}]"#;
    std::fs::write(chat_dir.join("run-state.json"), ignored_usage)
        .expect("failed to write sibling JSON file");
    std::fs::write(chat_dir.join("chat-messages.json.bak"), ignored_usage)
        .expect("failed to write backup file");
    std::fs::write(manicode.join("chat-messages.json"), ignored_usage)
        .expect("failed to write stray chat file");

    write_chat(
        &backup,
        "alpha",
        "2025-12-30T10-00-00.000Z",
        r#"[
  {"id":"a1","role":"assistant","timestamp":"2025-12-30T10:00:01.000Z","metadata":{"model":"claude-sonnet-4-20250514","usage":{"inputTokens":9999,"outputTokens":9999}},"credits":9.0},
  {"id":"a9","role":"assistant","timestamp":"2025-12-30T11:00:00.000Z","metadata":{"model":"claude-sonnet-4-20250514","usage":{"inputTokens":11,"outputTokens":7}}}
]"#,
    );

    write_chat(
        &manicode,
        "alpha",
        "2025-12-31T23-50-00.000Z",
        r#"[
  {"role":"assistant","metadata":{"model":"claude-haiku-4-5-20251001","usage":{"inputTokens":900,"outputTokens":400,"cachedTokensCreated":200,"cacheReadInputTokens":7000}},"credits":2.0},
  {"role":"assistant","timestamp":"2026-01-01T09:10:00+09:00","metadata":{"model":"claude-sonnet-4-20250514","usage":{"promptTokens":600,"completionTokens":100,"promptTokensDetails":{"cachedTokens":3000}}}}
]"#,
    );
    write_chat(
        &backup,
        "alpha",
        "2025-12-31T23-50-00.000Z",
        r#"[
  {"role":"assistant","metadata":{"model":"claude-haiku-4-5-20251001","usage":{"inputTokens":900,"outputTokens":400,"cachedTokensCreated":200,"cacheReadInputTokens":7000}},"credits":9.0},
  {"id":"same-time","role":"assistant","timestamp":"2026-01-01T09:10:00+09:00","metadata":{"model":"claude-sonnet-4-20250514","usage":{"promptTokens":600,"completionTokens":100,"promptTokensDetails":{"cachedTokens":3000}}}}
]"#,
    );

    let mtime_chat = write_chat(
        &manicode,
        "alpha",
        "untimed-chat",
        r#"[
  {"role":"assistant","timestamp":"yesterday","metadata":{"model":"claude-haiku-4-5-20251001","usage":{"inputTokens":50,"outputTokens":25}}},
  {"role":"assistant","timestamp":0,"metadata":{"model":"claude-haiku-4-5-20251001","usage":{"inputTokens":30,"outputTokens":15}}}
]"#,
    );
    set_modified(&mtime_chat, instant("2025-12-31T09:00:00Z"));

    write_chat(&manicode, "alpha", "empty-chat", "");
    write_chat(
        &manicode,
        "alpha",
        "truncated-chat",
        r#"[{"id":"t1","role":"assistant","timestamp":"2025-12-31T12:00:00.000Z","metadata":{"model":"claude-sonnet-4-20250514","usage":{"inputTokens":9999,"outp"#,
    );
    write_chat(
        &manicode,
        "alpha",
        "object-chat",
        r#"{"version":2,"messages":[{"id":"o1","role":"assistant","timestamp":"2025-12-31T12:00:00.000Z","metadata":{"model":"claude-sonnet-4-20250514","usage":{"inputTokens":6666,"outputTokens":6666}}}]}"#,
    );

    write_chat(
        &dev,
        "beta",
        "2026-01-02T10-00-00.000Z",
        r#"[
  {"id":"a1","role":"assistant","timestamp":"2026-01-02T10:00:00Z","metadata":{"model":"codebuff-mystery-1","usage":{"inputTokens":800,"outputTokens":300,"cacheCreationTokens":50,"cache_read_input_tokens":6000}}},
  {"id":"b2","variant":"agent","timestamp":"2026-01-02T11:00:00.000Z","metadata":{"runState":{"sessionState":{"mainAgentState":{"messageHistory":[
    {"role":"user","providerOptions":{"usage":{"inputTokens":9999,"outputTokens":9999}}},
    {"role":"assistant","providerOptions":{"usage":{"model":"claude-opus-4-20250514","inputTokens":8888,"outputTokens":8888,"cached_tokens_created":60}}},
    "garbage",
    {"role":"assistant"},
    {"role":"assistant","providerOptions":{"codebuff":{"model":"openai/gpt-5","usage":{"prompt_tokens":500,"completion_tokens":120,"prompt_tokens_details":{"cached_tokens":40}}}}}
  ]}}}}},
  {"id":"b3","role":"assistant","timestamp":"2026-01-02T11:30:00.000Z","metadata":{"model":"claude-sonnet-4-20250514","usage":{"inputTokens":100,"outputTokens":50,"totalTokens":500}}},
  {"id":"b4","role":"assistant","timestamp":"2026-01-02T12:00:00.000Z","credits":3.25},
  {"id":"b5","role":"assistant","timestamp":"2026-01-02T12:30:00.000Z","metadata":{"model":"grok-4","usage":{"inputTokens":40},"codebuff":{"usage":{"inputTokens":9999,"outputTokens":20,"credits":0.5}}},"credits":9.0}
]"#,
    );
    write_chat(
        &dev,
        "beta",
        "2025-12-27T22-00-00.000Z",
        r#"[{"role":"assistant","metadata":{"model":"claude-opus-4-20250514","usage":{"inputTokens":100,"outputTokens":50}}}]"#,
    );

    let mut padded = OsString::from(" ");
    padded.push(dev.join("projects"));
    padded.push(" ");
    let data_dirs = vec![
        backup.into_os_string(),
        padded,
        OsString::new(),
        manicode.clone().into_os_string(),
        manicode.into_os_string(),
        root.join("codebuff/missing").into_os_string(),
    ];
    vec![("CODEBUFF_DATA_DIR", data_dirs)]
}

pub(crate) fn seed_timestamp_ties(root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
    let store = root.join("codebuff");
    write_chat(
        &store.join("manicode"),
        "ties",
        "same-time",
        r#"[
      {"id":"tie-b","role":"assistant","timestamp":"2026-01-01T00:00:00Z","metadata":{"model":"claude-haiku-4-5-20251001","usage":{"inputTokens":20}}},
      {"id":"tie-a","role":"assistant","timestamp":"2026-01-01T00:00:00Z","metadata":{"model":"claude-sonnet-4-20250514","usage":{"inputTokens":100}},"credits":1.0},
      {"id":"tie-a","role":"assistant","timestamp":"2026-01-01T00:00:00Z","metadata":{"model":"claude-sonnet-4-20250514","usage":{"inputTokens":10}},"credits":2.75}
    ]"#,
    );
    vec![(
        "CODEBUFF_DATA_DIR",
        vec![store.join("manicode").into_os_string()],
    )]
}
