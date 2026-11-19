use std::ffi::OsString;
use std::path::{Path, PathBuf};

use ccusage_test_support::{instant, set_modified};

const PRO: &str = "gemini-2.5-pro";
const FLASH: &str = "gemini-2.5-flash";
const FLASH_PREVIEW: &str = "gemini-3-flash-preview";
const SONNET: &str = "claude-sonnet-4-20250514";
const UNPRICED: &str = "gemini-mystery-1";

fn seed_headless_stats_variants(write: &impl Fn(&str, &str) -> PathBuf) {
    write(
        "tmp/proj-beta/chats/headless-eeee5555.json",
        &format!(
            r#"{{
  "session_id": "eeee5555-headless",
  "timestamp": "2025-12-31T18:00:00.000Z",
  "response": "ok",
  "result": {{"stats": {{"input": 9999, "output": 9999}}}},
  "stats": {{
    "models": {{
      "{PRO}": {{"api": {{"totalRequests": 2}}, "tokens": {{"prompt":1200,"candidates":150,"cached":200,"thoughts":40,"tool":8,"total":1398}}}},
      "{UNPRICED}": {{"tokens": {{"input":100,"output":20,"cached":500}}}},
      "gemini-empty": {{"tokens": {{"input":0}}}},
      "gemini-no-tokens": {{"api": {{"totalRequests": 1}}}}
    }},
    "tools": {{"totalCalls": 3}},
    "input": 9999
  }}
}}"#
        ),
    );
    let result = write(
        "tmp/proj-beta/chats/result-ffff6666.json",
        &format!(
            r#"{{"model":"{FLASH_PREVIEW}","result":{{"stats":{{"input_tokens":300,"output_tokens":45,"cached_tokens":100,"reasoning_tokens":5,"total_tokens":350}}}}}}"#
        ),
    );
    set_modified(&result, instant("2026-01-01T15:00:00Z"));
    write(
        "tmp/proj-beta/chats/fallback-gggg7777.json",
        &format!(
            r#"{{"timestamp":"2025-12-30T20:00:00.000Z","stats":{{"models":{{"{PRO}":{{"tokens":{{}}}}}},"input":33,"output":11}}}}"#
        ),
    );
}

pub(crate) fn seed(root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
    let store = root.join("gemini");
    let write = |relative: &str, contents: &str| {
        let path = store.join(relative);
        std::fs::create_dir_all(path.parent().expect("fixture file has a parent"))
            .expect("failed to create Gemini fixture directory");
        std::fs::write(&path, contents).expect("failed to write Gemini fixture file");
        path
    };

    write(
        "tmp/proj-alpha/chats/session-2025-12-30T09-00-aaaa1111.json",
        &format!(
            r#"{{
  "sessionId": "aaaa1111-session-json",
  "projectHash": "proj-alpha",
  "startTime": "2025-12-30T09:00:00.000Z",
  "lastUpdated": "2025-12-31T16:00:00.000Z",
  "messages": [
    {{"id":"a-u1","type":"user","timestamp":"2025-12-30T09:00:00.000Z","content":"hi","tokens":{{"input":9999,"output":9999,"total":19998}}}},
    {{"id":"a-m1","type":"gemini","timestamp":"2025-12-30T09:00:05.000Z","model":"{PRO}","tokens":{{"input":1000,"output":200,"cached":400,"thoughts":50,"tool":10,"total":1260}}}},
    {{"id":"a-m2","type":"gemini","timestamp":"2025-12-30T09:10:00Z","model":"{FLASH}","tokens":{{"input":800,"output":100,"cached":300,"thoughts":0,"tool":0,"total":1200}}}},
    {{"id":"a-m2","type":"gemini","timestamp":"2025-12-30T09:10:00Z","model":"{FLASH}","tokens":{{"input":800,"output":100,"cached":300,"thoughts":0,"tool":0,"total":1200}}}},
    {{"id":"a-m3","type":"gemini","timestamp":"2025-12-31T15:30:00.000Z","model":"{FLASH_PREVIEW}","tokens":{{"prompt":500,"candidates":120,"cached_tokens":80,"reasoning":30,"tool_tokens":5}}}},
    {{"id":"a-m4","type":"gemini","created_at":"2025-12-31T16:00:00+00:00","model":"{UNPRICED}","tokens":{{"total":654}}}},
    {{"id":"a-m5","type":"gemini","timestamp":"yesterday","model":"{PRO}","tokens":{{"input":70,"output":30}}}},
    {{"id":"a-m6","type":"gemini","timestamp":"2025-12-31T12:00:00.000Z","tokens":{{"input":8888,"output":8888}}}},
    {{"id":"a-m7","type":"gemini","timestamp":"2025-12-31T12:00:00.000Z","model":"{PRO}","tokens":{{"input":0,"output":0,"cached":0,"total":0}}}},
    {{"id":"a-m8","type":"gemini","timestamp":"2025-12-31T12:00:00.000Z","model":"{PRO}","tokens":"9999"}},
    {{"id":"a-m9","type":"gemini","timestamp":"2026-01-01T00:30:00.000Z","model":"{FLASH}","tokens":{{"input":12.9,"output":-5,"cached":3.2,"total":15.7}}}},
    {{"id":"a-m10","type":"gemini","timestamp":"2026-01-01T10:00:00.000Z","model":"{SONNET}","tokens":{{"input":100,"output":40,"total":200}}}},
    {{"id":"a-m11","type":" gemini ","timestamp":"2026-01-01T10:00:00.000Z","model":"{PRO}","tokens":{{"input":7777,"output":7777}}}},
    "garbage",
    42
  ]
}}"#
        ),
    );

    let without_id = format!(
        r#"{{"type":"gemini","timestamp":"2025-12-31T23:59:59.999Z","model":"{FLASH_PREVIEW}","tokens":{{"input":40,"output":10}}}}"#
    );
    let jsonl = write(
        "tmp/proj-alpha/chats/session-bbbb2222.jsonl",
        &[
            r#"{"sessionId":"bbbb2222-session-jsonl","projectHash":"proj-alpha","startTime":"2025-12-31T14:00:00.000Z","kind":"main"}"#.to_string(),
            r#"{"id":"b-u1","type":"user","timestamp":"2025-12-31T14:00:00.000Z","content":"hi","tokens":{"input":9999,"output":9999,"total":19998}}"#.to_string(),
            format!(r#"{{"id":"b-m1","type":"gemini","timestamp":"2025-12-31T14:00:10.000Z","model":"{PRO}","tokens":{{"input":7777,"output":7777,"total":15554}}}}"#),
            format!(r#"{{"id":"b-m1","type":"gemini","timestamp":"2025-12-31T14:59:59.000Z","model":"{PRO}","tokens":{{"input":2000,"output":300,"cached":1500,"thoughts":100,"tool":0,"total":2400}}}}"#),
            format!(r#"{{"id":"b-m1","type":"gemini","timestamp":"2025-12-31T15:00:30.000Z","model":"{PRO}"}}"#),
            format!(r#"{{"id":"b-m1","type":"gemini","timestamp":"2025-12-31T15:00:40.000Z","model":"{PRO}","tokens":{{"input":0,"output":0,"total":0}}}}"#),
            r#"{"id":"b-m2","type":"gemini","timestamp":"2025-12-31T15:00:00.000Z","tokens":{"input":300,"output":60,"total":360}}"#.to_string(),
            without_id.clone(),
            without_id,
            r#"{"$set":{"lastUpdated":"2025-12-31T23:59:59.999Z"}}"#.to_string(),
            format!(r#"{{"type":"info","model":"{UNPRICED}"}}"#),
            r#"{"id":"b-m3","type":"gemini","timestamp":"2026-01-01T09:00:00.000Z","tokens":{"input":250,"output":50,"cached":0,"total":300}}"#.to_string(),
            format!(r#"{{"id":"b-bad","type":"gemini","timestamp":"2026-01-01T09:30:00.000Z","model":"{PRO}","tokens":{{"input":9999,"outp"#),
            String::new(),
            format!(r#"{{"type":" gemini ","model":"{FLASH}","tokens":{{"input":7777,"output":7777}}}}"#),
            r#"{"sessionId":"bbbb2222-renamed","id":"b-m4","type":"gemini","timestamp":"2026-01-02T08:00:00.000Z","tokens":{"input":150,"output":25}}"#.to_string(),
            r#"{"timestamp":"2026-01-02T09:00:00.000Z","stats":{"input_tokens":90,"output_tokens":20,"cached":30}}"#.to_string(),
            r#"{"id":"b-m5","type":"gemini","tokens":{"input":60,"output":15}}"#.to_string(),
            format!(r#"{{"id":"b-m6","type":"gemini","timestamp":"2026-01-02T13:00:00.000Z","model":"{PRO}","tokens":{{"input":9999,"output":99"#),
        ]
        .join("\n"),
    );
    set_modified(&jsonl, instant("2026-01-02T12:00:00Z"));

    write(
        "tmp/proj-beta/chats/cccc3333.jsonl",
        &[
            format!(r#"{{"id":"b-m1","type":"gemini","timestamp":"2026-01-01T12:00:00.000Z","model":"{PRO}","tokens":{{"input":450000,"output":1000,"cached":150000,"total":451000}}}}"#),
            r#"{"session_id":"cccc3333-snake"}"#.to_string(),
            format!(r#"{{"id":"c-2","type":"gemini","timestamp":"2026-01-02T23:30:00.000Z","model":"{SONNET}","tokens":{{"input":500,"output":80,"cached":2000,"total":2580}}}}"#),
            String::new(),
        ]
        .join("\n"),
    );

    write(
        "tmp/proj-beta/chats/session-dddd4444.json",
        &format!(
            r#"{{"type":"gemini","id":"d-1","session_id":"dddd4444-snake","sessionId":"dddd4444-camel","startTime":"2025-12-30T00:00:00.000Z","created_at":"2026-01-02T10:00:00Z","model":"{FLASH}","tokens":{{"input":400,"output":90,"thoughts":10}}}}"#
        ),
    );
    let stale = write(
        "tmp/proj-beta/chats/session-hhhh8888.json",
        &format!(
            r#"{{"type":"gemini","id":"h-1","startTime":"2025-12-30T00:00:00.000Z","model":"{SONNET}","tokens":{{"input":100,"output":50}}}}"#
        ),
    );
    set_modified(&stale, instant("2025-12-27T22:00:00Z"));

    seed_headless_stats_variants(&write);

    write(
        "settings-backup/shadow.json",
        &format!(
            r#"{{"sessionId":"shadow-outside-chats","messages":[{{"id":"s-1","type":"gemini","timestamp":"2025-12-30T12:00:00.000Z","model":"{FLASH}","tokens":{{"input":21,"output":4}}}}]}}"#
        ),
    );

    write("tmp/proj-beta/chats/empty.json", "");
    write("tmp/proj-beta/chats/empty.jsonl", "");
    write(
        "tmp/proj-beta/chats/truncated.json",
        &format!(
            r#"{{"sessionId":"truncated","messages":[{{"id":"t-1","type":"gemini","timestamp":"2025-12-31T12:00:00.000Z","model":"{PRO}","tokens":{{"input":9999,"outp"#
        ),
    );
    write(
        "tmp/proj-beta/logs.json",
        r#"[{"sessionId":"cccc3333","messageId":0,"type":"user","message":"hi","timestamp":"2025-12-31T12:00:00.000Z","tokens":{"input":5555,"output":5555}}]"#,
    );
    write(
        "tmp/proj-beta/chats/metadata.json",
        r#"{"version":1,"kind":"metadata","tokens":{"input":6666,"output":6666}}"#,
    );
    write(
        "tmp/proj-beta/chats/notes.txt",
        &format!(r#"{{"type":"gemini","model":"{PRO}","tokens":{{"input":4444,"output":4444}}}}"#),
    );

    vec![("GEMINI_DATA_DIR", vec![store.into_os_string()])]
}
