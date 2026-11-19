use std::ffi::OsString;
use std::path::{Path, PathBuf};

use ccusage_test_support::{instant, set_modified};

const SONNET: &str = "claude-sonnet-4-20250514";
const OPUS: &str = "claude-opus-4-20250514";
const HAIKU: &str = "claude-haiku-4-5-20251001";
const UNPRICED: &str = "droid-mystery-1";

pub(crate) fn seed(root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
    let sessions = root.join("droid/sessions");
    let archive = root.join("droid/archive");
    let alpha = sessions.join("-Users-dev-alpha");
    let beta = sessions.join("-Users-dev-beta");
    for dir in [&alpha, &beta, &archive] {
        std::fs::create_dir_all(dir).expect("failed to create Droid sessions directory");
    }
    let write = |dir: &Path, name: &str, contents: &str| -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, contents).expect("failed to write Droid session file");
        path
    };

    write(
        &alpha,
        "stale-0001.settings.json",
        r#"{
  "model": "custom:Claude-Sonnet-4-20250514-[Anthropic]",
  "providerLock": "anthropic",
  "providerLockTimestamp": "2025-12-27T22:00:00.000Z",
  "autonomyMode": "auto-low",
  "tokenUsage": {"inputTokens": 100, "outputTokens": 50}
}"#,
    );
    write(
        &alpha,
        "opus-0002.settings.json",
        &format!(
            r#"{{
  "model": "{OPUS}",
  "providerLock": "claude",
  "providerLockTimestamp": "2025-12-30T10:00:00.000Z",
  "tokenUsage": {{"inputTokens": 1000, "outputTokens": 200, "cacheCreationTokens": 300, "cacheReadTokens": 4000, "thinkingTokens": 60}}
}}"#
        ),
    );

    write(
        &archive,
        "gpt-0003.settings.json",
        r#"{"model":"gpt-5","providerLock":"openai","providerLockTimestamp":"2025-12-30T08:00:00.000Z","tokenUsage":{"inputTokens":9999,"outputTokens":9999,"cacheReadTokens":9999}}"#,
    );
    write(
        &alpha,
        "gpt-0003.settings.json",
        r#"{"model":"gpt-5","providerLock":"openai","providerLockTimestamp":"2025-12-31T15:30:00Z","tokenUsage":{"inputTokens":700,"outputTokens":150,"cacheReadTokens":2500}}"#,
    );
    write(
        &beta,
        "gpt-0003-fork.settings.json",
        r#"{"model":"gpt-5","providerLock":"openai","providerLockTimestamp":"2025-12-31T15:45:00.000Z","tokenUsage":{"inputTokens":70,"outputTokens":15}}"#,
    );

    write(
        &beta,
        "mystery-0004.settings.json",
        &format!(
            r#"{{"model":"{UNPRICED}","providerLockTimestamp":"2025-12-31T10:00:00.000Z","tokenUsage":{{"inputTokens":300,"outputTokens":70,"cacheCreationTokens":20,"cacheReadTokens":500,"thinkingTokens":10}}}}"#
        ),
    );
    write(
        &beta,
        "offset-0005.settings.json",
        r#"{"model":"gemini-2.5-pro","providerLock":"vertex-ai","providerLockTimestamp":"2026-01-01T08:30:00+09:00","tokenUsage":{"inputTokens":400,"outputTokens":90}}"#,
    );

    write(
        &alpha,
        "sidecar-0006.settings.json",
        r#"{"providerLockTimestamp":"2026-01-01T00:10:00.000Z","tokenUsage":{"inputTokens":900,"outputTokens":400,"cacheCreationTokens":200,"cacheReadTokens":7000}}"#,
    );
    write(
        &alpha,
        "sidecar-0006.jsonl",
        &format!(
            r#"{{"type":"session_start","id":"sidecar-0006","title":"truncate
{{"type":"message","message":{{"role":"user","content":[{{"type":"text","text":"hello"}}]}}}}
{{"type":"message","message":{{"role":"user","content":[{{"type":"text","text":"<system-reminder>\nModel: {HAIKU} [Anthropic]\n</system-reminder>"}}]}}}}
{{"type":"message","message":{{"role":"user","content":[{{"type":"text","text":"Model: gpt-5"}}]}}}}
"#
        ),
    );

    write(
        &beta,
        "default-0007.settings.json",
        r#"{"model":"  ","providerLock":"OpenAI","providerLockTimestamp":"2026-01-01T12:00:00.000Z","tokenUsage":{"totalTokens":345}}"#,
    );
    write(
        &beta,
        "bracket-0008.settings.json",
        r#"{"model":"[Anthropic]","providerLock":"grok","providerLockTimestamp":"2026-01-01T13:00:00.000Z","tokenUsage":{"inputTokens":40,"outputTokens":10,"totalTokens":80}}"#,
    );

    write(
        &alpha,
        "tie-0009.settings.json",
        &format!(
            r#"{{"model":"{SONNET}","providerLock":"anthropic","providerLockTimestamp":"2026-01-01T18:00:00.000Z","tokenUsage":{{"inputTokens":8888,"outputTokens":8888}}}}"#
        ),
    );
    write(
        &beta,
        "tie-0009.settings.json",
        &format!(
            r#"{{"model":"{SONNET}","providerLock":"anthropic","providerLockTimestamp":"2026-01-01T18:00:00.000Z","tokenUsage":{{"inputTokens":600,"outputTokens":100}}}}"#
        ),
    );

    write(
        &archive,
        "late-0010.settings.json",
        &format!(
            r#"{{"model":"{SONNET}","providerLock":"anthropic","providerLockTimestamp":"2026-01-01T10:00:00.000Z","tokenUsage":{{"inputTokens":7777,"outputTokens":7777}}}}"#
        ),
    );
    write(
        &alpha,
        "late-0010.settings.json",
        &format!(
            r#"{{"model":"{SONNET}","providerLock":"anthropic","providerLockTimestamp":"2026-01-02T09:00:00.000Z","tokenUsage":{{"inputTokens":800,"outputTokens":300,"cacheReadTokens":6000}}}}"#
        ),
    );

    let mtime_only = write(
        &beta,
        "mtime-0011.settings.json",
        &format!(
            r#"{{"model":"{SONNET}","providerLock":"anthropic","providerLockTimestamp":"2026-01-02 11:00:00","tokenUsage":{{"inputTokens":500,"outputTokens":120}}}}"#
        ),
    );
    set_modified(&mtime_only, instant("2026-01-02T12:00:00Z"));

    write(
        &alpha,
        "zero-0012.settings.json",
        r#"{"model":"gpt-5","providerLock":"openai","providerLockTimestamp":"2025-12-31T12:00:00.000Z","tokenUsage":{"inputTokens":0,"outputTokens":0}}"#,
    );
    write(
        &alpha,
        "nousage-0013.settings.json",
        r#"{"model":"gpt-5","providerLock":"openai","providerLockTimestamp":"2025-12-31T12:00:00.000Z"}"#,
    );
    write(
        &alpha,
        "nonnumeric-0014.settings.json",
        r#"{"model":"gpt-5","providerLock":"openai","providerLockTimestamp":"2025-12-31T12:00:00.000Z","tokenUsage":{"inputTokens":"9999","outputTokens":9999.5,"cacheReadTokens":-9999}}"#,
    );
    write(
        &alpha,
        "scalarusage-0015.settings.json",
        r#"{"model":"gpt-5","providerLockTimestamp":"2025-12-31T12:00:00.000Z","tokenUsage":9999}"#,
    );
    write(
        &beta,
        "array-0016.settings.json",
        r#"[{"model":"gpt-5","providerLockTimestamp":"2025-12-31T12:00:00.000Z","tokenUsage":{"inputTokens":9999,"outputTokens":9999}}]"#,
    );
    write(
        &beta,
        "truncated-0017.settings.json",
        r#"{"model":"gpt-5","providerLockTimestamp":"2025-12-31T12:00:00.000Z","tokenUsage":{"inputTokens":9999,"outp"#,
    );
    write(&beta, "empty-0018.settings.json", "");
    write(
        &beta,
        "settings.json",
        r#"{"model":"gpt-5","providerLockTimestamp":"2025-12-31T12:00:00.000Z","tokenUsage":{"inputTokens":5555,"outputTokens":5555}}"#,
    );
    write(
        &beta,
        "backup-0019.settings.json.bak",
        r#"{"model":"gpt-5","providerLockTimestamp":"2025-12-31T12:00:00.000Z","tokenUsage":{"inputTokens":6666,"outputTokens":6666}}"#,
    );
    write(
        &beta,
        "orphan-0020.jsonl",
        "{\"type\":\"message\",\"message\":{\"content\":\"Model: gpt-5\"}}\n",
    );

    let mut padded = OsString::from(" ");
    padded.push(archive);
    let roots = vec![
        sessions.clone().into_os_string(),
        padded,
        sessions.into_os_string(),
        root.join("droid/missing").into_os_string(),
    ];
    vec![("DROID_SESSIONS_DIR", roots)]
}
