use std::ffi::OsString;
use std::path::Path;

const SONNET: &str = "claude-sonnet-4-20250514";
const HAIKU: &str = "claude-haiku-4-5-20251001";
const UNPRICED: &str = "amp-mystery-1";

fn seed_ledger_with_cache_split_from_messages(write: &impl Fn(&str, &str)) {
    let replayed_event = r#"{"id":"event-a2","timestamp":"2025-12-31T15:30:00.000Z","model":"gpt-5","tokens":{"input":700,"output":150},"toMessageId":3,"credits":0.75}"#;
    write(
        "T-aaaaaaaa-0000-4000-8000-000000000001.json",
        &format!(
            r#"{{
  "id": "T-aaaaaaaa-0000-4000-8000-000000000001",
  "title": "Ledger thread",
  "messages": [
    {{"role":"user","messageId":0,"content":"hi"}},
    {{"role":"assistant","messageId":1,"usage":{{"model":"{SONNET}","inputTokens":9999,"outputTokens":9999,"cacheCreationInputTokens":300,"cacheReadInputTokens":4000,"timestamp":"2025-12-30T10:00:00.000Z"}}}},
    {{"role":"assistant","messageId":3,"usage":{{"model":"gpt-5","cacheReadInputTokens":2500}}}}
  ],
  "usageLedger": {{"events": [
    {{"id":"event-a1","timestamp":"2025-12-30T10:00:00.000Z","model":"{SONNET}","tokens":{{"input":1000,"output":200}},"toMessageId":1,"credits":1.5}},
    {replayed_event},
    {replayed_event},
    {{"id":"event-a3","timestamp":"2026-01-01T12:00:00.000Z","model":"{UNPRICED}","tokens":{{"input":300,"output":70}},"credits":0.2}},
    {{"id":"event-a4","timestamp":"2026-01-01T12:30:00.000Z","model":"gpt-5","tokens":{{"total":345}}}},
    {{"id":"event-no-timestamp","model":"gpt-5","tokens":{{"input":8888,"output":8888}}}},
    {{"id":"event-no-tokens","timestamp":"2026-01-01T13:00:00.000Z","model":"gpt-5","tokens":{{"input":0,"output":0}}}}
  ]}}
}}"#
        ),
    );
}

fn seed_assistant_usage_without_ledger(write: &impl Fn(&str, &str)) {
    write(
        "T-bbbbbbbb-0000-4000-8000-000000000002.json",
        &format!(
            r#"{{
  "id": "T-bbbbbbbb-0000-4000-8000-000000000002",
  "usageLedger": {{}},
  "messages": [
    {{"role":"user","content":"hi","usage":{{"model":"{SONNET}","inputTokens":7777,"outputTokens":7777,"timestamp":"2025-12-31T23:49:00.000Z"}}}},
    "garbage",
    {{"role":"assistant","messageId":2,"usage":{{"model":"{HAIKU}","inputTokens":900,"outputTokens":400,"cacheCreationInputTokens":200,"cacheReadInputTokens":7000,"credits":2.0,"timestamp":"2025-12-31T23:50:00.000Z"}}}},
    {{"role":"assistant","messageId":4,"usage":{{"model":"{SONNET}","inputTokens":600,"outputTokens":100,"cacheReadInputTokens":3000,"timestamp":"2026-01-01T00:10:00.000Z"}}}},
    {{"role":"assistant","messageId":6,"usage":{{"model":"{SONNET}","inputTokens":0,"outputTokens":0,"timestamp":"2026-01-01T00:20:00.000Z"}}}},
    {{"role":"assistant","messageId":8,"timestamp":"2026-01-02T10:00:00.000Z","model":"{UNPRICED}","usage":{{"inputTokens":800,"outputTokens":300,"cacheCreationInputTokens":50,"cacheReadInputTokens":6000}}}},
    {{"role":"assistant","messageId":10,"usage":{{"model":"{SONNET}","totalTokens":500,"timestamp":"2026-01-02T11:00:00.000Z"}}}},
    {{"role":"assistant","messageId":12,"usage":{{"model":"{SONNET}","inputTokens":6666,"outputTokens":6666}}}}
  ]
}}"#
        ),
    );
}

pub(crate) fn seed(root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
    let threads = root.join("amp/threads");
    std::fs::create_dir_all(&threads).expect("failed to create Amp threads directory");
    let write = |name: &str, contents: &str| {
        std::fs::write(threads.join(name), contents).expect("failed to write Amp thread file");
    };

    seed_ledger_with_cache_split_from_messages(&write);

    seed_assistant_usage_without_ledger(&write);

    write(
        "T-cccccccc-0000-4000-8000-000000000003.json",
        &format!(
            r#"{{"id":"T-cccccccc-0000-4000-8000-000000000003","messages":[{{"role":"assistant","messageId":1,"usage":{{"model":"{SONNET}","inputTokens":100,"outputTokens":50,"timestamp":"2025-12-27T22:00:00.000Z"}}}}]}}"#
        ),
    );

    write("T-empty.json", "");
    write(
        "T-truncated.json",
        &format!(
            r#"{{"id":"T-truncated","messages":[{{"role":"assistant","usage":{{"model":"{SONNET}","inputTokens":9999,"outp"#
        ),
    );
    write(
        "settings.json",
        &format!(
            r#"{{"version":3,"kind":"settings","messages":[{{"role":"assistant","usage":{{"model":"{SONNET}","inputTokens":5555,"outputTokens":5555,"timestamp":"2025-12-31T12:00:00.000Z"}}}}]}}"#
        ),
    );
    write(
        "index.json",
        r#"["T-aaaaaaaa-0000-4000-8000-000000000001"]"#,
    );
    write("notes.txt", "not a thread\n");

    vec![("AMP_DATA_DIR", vec![root.join("amp").into_os_string()])]
}
