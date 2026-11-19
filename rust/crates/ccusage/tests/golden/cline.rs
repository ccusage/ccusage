use std::ffi::OsString;
use std::path::Path;

use ccusage_test_support::instant;

const SONNET: &str = "claude-sonnet-4-20250514";
const OPUS: &str = "claude-opus-4-20250514";
const HAIKU: &str = "claude-haiku-4-5-20251001";
const UNPRICED: &str = "cline-mystery-1";

fn say(say: &str, ts: &str, model_info: &str, text: &str) -> String {
    let text = text.replace('\\', "\\\\").replace('"', "\\\"");
    format!(r#"{{"ts":{ts},"type":"say","say":"{say}","modelInfo":{model_info},"text":"{text}"}}"#)
}

fn request(ts: i64, model_info: &str, text: &str) -> String {
    say("api_req_started", &ts.to_string(), model_info, text)
}

fn extension_model(model: &str, provider: &str) -> String {
    format!(r#"{{"modelId":"{model}","providerId":"{provider}","mode":"act"}}"#)
}

fn document(messages: &[String]) -> String {
    format!("[\n  {}\n]\n", messages.join(",\n  "))
}

fn transcript(messages: &[String]) -> String {
    format!(
        "{{\"version\":1,\"messages\":[\n  {}\n]}}\n",
        messages.join(",\n  ")
    )
}

pub(crate) fn seed(root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
    let store = root.join("cline");
    let write = |relative: &str, contents: &str| {
        let path = store.join(relative);
        std::fs::create_dir_all(path.parent().expect("fixture file has a parent"))
            .expect("failed to create Cline fixture directory");
        std::fs::write(path, contents).expect("failed to write Cline fixture file");
    };

    let sonnet = extension_model(SONNET, "anthropic");
    let ignored =
        r#"{"tokensIn":9999,"tokensOut":9999,"cacheReads":9999,"cacheWrites":9999,"cost":9.99}"#;

    let replayed = request(
        instant("2025-12-31T15:30:00Z").millis(),
        &extension_model(OPUS, "anthropic"),
        r#"{"requestId":"req-replayed","tokensIn":700,"tokensOut":150,"cacheReads":2500,"cacheWrites":0}"#,
    );
    write(
        "tasks/task-alpha/ui_messages.json",
        &document(&[
            format!(
                r#"{{"ts":{ts},"type":"say","say":"text","text":"hello"}}"#,
                ts = instant("2025-12-30T09:58:20Z").millis()
            ),
            request(
                instant("2025-12-30T10:00:00Z").millis(),
                &sonnet,
                r#"{"request":"first","tokensIn":1000,"tokensOut":200,"cacheReads":4000,"cacheWrites":300,"cost":0.5}"#,
            ),
            replayed.clone(),
            replayed,
            request(
                instant("2026-01-01T12:00:00Z").millis(),
                &extension_model(UNPRICED, "mystery"),
                r#"{"tokensIn":300,"tokensOut":70,"cacheReads":0,"cacheWrites":0}"#,
            ),
            request(
                instant("2026-01-01T12:30:00Z").millis(),
                &extension_model("gpt-5", "openai"),
                r#"{"tokensIn":400,"tokensOut":45,"cacheReads":100,"cacheWrites":0,"cost":"0.25"}"#,
            ),
            request(
                instant("2026-01-02T09:00:00Z").millis(),
                &extension_model(HAIKU, "anthropic"),
                r#"{"tokensIn":800,"tokensOut":300,"cacheReads":6000,"cacheWrites":50,"cost":-1}"#,
            ),
            format!(
                r#"{{"ts":{},"type":"say","say":"api_req_started","partial":true,"modelInfo":{sonnet},"text":"{}"}}"#,
                instant("2025-12-31T12:00:00Z").millis(),
                ignored.replace('"', "\\\"")
            ),
            format!(
                r#"{{"ts":{},"type":"ask","ask":"api_req_started","modelInfo":{sonnet},"text":"{}"}}"#,
                instant("2025-12-31T12:01:00Z").millis(),
                ignored.replace('"', "\\\"")
            ),
            say(
                "deleted_api_reqs",
                &instant("2025-12-31T12:02:00Z").millis().to_string(),
                &sonnet,
                r#"{"tokensIn":8888,"tokensOut":8888,"cacheReads":8888,"cacheWrites":8888,"cost":8.88}"#,
            ),
            say(
                "subagent_usage",
                &instant("2025-12-31T12:03:00Z").millis().to_string(),
                &sonnet,
                r#"{"tokensIn":7777,"tokensOut":7777,"cacheReads":7777,"cacheWrites":7777,"cost":7.77}"#,
            ),
            request(
                instant("2025-12-31T12:04:00Z").millis(),
                &sonnet,
                r#"{"tokensIn":6666,"tokensOut":6666,"cacheReads":6666,"cost":6.66}"#,
            ),
            request(
                instant("2025-12-31T12:05:00Z").millis(),
                &sonnet,
                r#"{"tokensIn":"9999","tokensOut":9999,"cacheReads":9999,"cacheWrites":9999}"#,
            ),
            request(
                instant("2025-12-31T12:06:00Z").millis(),
                &sonnet,
                r#"{"tokensIn":9999.5,"tokensOut":9999,"cacheReads":9999,"cacheWrites":9999}"#,
            ),
            request(
                instant("2025-12-31T12:07:00Z").millis(),
                &sonnet,
                r#"{"tokensIn":0,"tokensOut":0,"cacheReads":0,"cacheWrites":0,"cost":5.55}"#,
            ),
            request(instant("2025-12-31T12:08:00Z").millis(), "null", ignored),
            request(
                instant("2025-12-31T12:09:00Z").millis(),
                &extension_model("UNKNOWN", "anthropic"),
                ignored,
            ),
            request(
                instant("2025-12-31T12:10:00Z").millis(),
                r#"{"id":"claude-sonnet-4-20250514"}"#,
                ignored,
            ),
            say(
                "api_req_started",
                &format!("\"{ts}\"", ts = instant("2025-12-31T12:00:00Z").millis()),
                &sonnet,
                ignored,
            ),
            say(
                "api_req_started",
                &format!(
                    "{ts}",
                    ts = instant("2025-12-31T12:00:00.0005Z").millis_f64()
                ),
                &sonnet,
                ignored,
            ),
            say("api_req_started", "null", &sonnet, ignored),
            request(
                instant("2025-12-31T12:11:00Z").millis(),
                &sonnet,
                r#"{"tokensIn":9999,"tokensOut""#,
            ),
            request(
                instant("2025-12-31T12:12:00Z").millis(),
                &sonnet,
                "[9999,9999,9999,9999]",
            ),
            format!(
                r#"{{"ts":{},"type":"say","say":"api_req_started","modelInfo":{sonnet},"text":{{"tokensIn":9999,"tokensOut":9999,"cacheReads":9999,"cacheWrites":9999}}}}"#,
                instant("2025-12-31T12:13:00Z").millis()
            ),
            "\"garbage\"".to_string(),
        ]),
    );
    write(
        "tasks/task-alpha/task_metadata.json",
        &format!(
            r#"{{"model_usage":[{{"ts":{},"model_id":"{SONNET}","model_provider_id":"anthropic","mode":"act"}}]}}"#,
            instant("2025-12-31T12:00:00Z").millis()
        ),
    );
    write(
        "tasks/task-alpha/api_conversation_history.json",
        r#"[{"role":"assistant","content":[],"metrics":{"inputTokens":9999,"outputTokens":9999,"cacheReadTokens":0,"cacheWriteTokens":0}}]"#,
    );

    write(
        "data/tasks/task-alpha/ui_messages.json",
        &document(&[request(
            instant("2025-12-31T23:50:00Z").millis(),
            r#"{"modelId":"anthropic/claude-sonnet-4","providerId":"openrouter"}"#,
            r#"{"id":"legacy-1","tokensIn":900,"tokensOut":400,"cacheReads":7000,"cacheWrites":200,"cost":2.0}"#,
        )]),
    );

    let cli = |head: &str, model: &str, metrics: &str| {
        format!(r#"{{"role":"assistant",{head},"modelInfo":{model},"metrics":{metrics}}}"#)
    };
    let cli_sonnet = format!(r#"{{"id":"{SONNET}","provider":"anthropic"}}"#);
    let cli_ignored = r#"{"inputTokens":9999,"outputTokens":9999,"cacheReadTokens":0,"cacheWriteTokens":0,"cost":9.99}"#;
    write(
        "data/sessions/task-alpha/task-alpha.messages.json",
        &transcript(&[
            format!(
                r#"{{"role":"user","ts":{},"modelInfo":{cli_sonnet},"metrics":{cli_ignored}}}"#,
                instant("2025-12-31T15:59:00Z").millis()
            ),
            cli(
                &format!(
                    r#""ts":{},"id":"assistant-1""#,
                    instant("2025-12-31T16:00:00Z").millis()
                ),
                &cli_sonnet,
                r#"{"inputTokens":5000,"outputTokens":600,"cacheReadTokens":3000,"cacheWriteTokens":500,"cost":0}"#,
            ),
            cli(
                r#""timestamp":"2026-01-01T00:10:00Z","messageId":"assistant-2""#,
                &format!(r#"{{"id":"{HAIKU}","provider":"anthropic"}}"#),
                r#"{"inputTokens":3600,"outputTokens":100,"cacheReadTokens":3000,"cacheWriteTokens":0}"#,
            ),
            cli(
                r#""timestamp":"2026-01-01T09:20:00.123+09:00","requestId":"assistant-3""#,
                &format!(r#"{{"id":"{UNPRICED}"}}"#),
                r#"{"inputTokens":250,"outputTokens":50,"cacheReadTokens":0,"cacheWriteTokens":0,"cost":0.07}"#,
            ),
            cli(
                r#""ts":"invalid","timestamp":"2025-12-31T12:00:00Z""#,
                &cli_sonnet,
                cli_ignored,
            ),
            cli(
                r#""timestamp":"2025-12-31T12:00:00""#,
                &cli_sonnet,
                cli_ignored,
            ),
            cli(
                r#""timestamp":"2025-12-31T12:00:00.5Z""#,
                &cli_sonnet,
                cli_ignored,
            ),
            cli(r#""id":"no-timestamp""#, &cli_sonnet, cli_ignored),
            cli(
                &format!(r#""ts":{}"#, instant("2025-12-31T12:01:00Z").millis()),
                &cli_sonnet,
                r#"{"inputTokens":5,"outputTokens":8888,"cacheReadTokens":4,"cacheWriteTokens":2,"cost":8.88}"#,
            ),
            cli(
                &format!(r#""ts":{}"#, instant("2025-12-31T12:02:00Z").millis()),
                &cli_sonnet,
                r#"{"inputTokens":7777,"outputTokens":7777,"cacheReadTokens":0,"cost":7.77}"#,
            ),
            cli(
                &format!(r#""ts":{}"#, instant("2025-12-31T12:03:00Z").millis()),
                r#"{"id":"unknown"}"#,
                cli_ignored,
            ),
            cli(
                &format!(r#""ts":{}"#, instant("2025-12-31T12:04:00Z").millis()),
                r#"{"modelId":"claude-sonnet-4-20250514"}"#,
                cli_ignored,
            ),
            cli(
                &format!(r#""ts":{}"#, instant("2025-12-31T12:05:00Z").millis()),
                &cli_sonnet,
                r#"{"inputTokens":0,"outputTokens":0,"cacheReadTokens":0,"cacheWriteTokens":0,"cost":5.55}"#,
            ),
            format!(
                r#"{{"role":"assistant","ts":{},"modelInfo":{cli_sonnet}}}"#,
                instant("2025-12-31T12:06:00Z").millis()
            ),
            format!(
                r#"{{"role":"tool","ts":{},"modelInfo":{cli_sonnet},"metrics":{cli_ignored}}}"#,
                instant("2025-12-31T12:07:00Z").millis()
            ),
            "\"garbage\"".to_string(),
        ]),
    );

    let first_beta = cli(
        &format!(
            r#""ts":{},"id":"beta-1""#,
            instant("2025-12-30T22:00:00Z").millis()
        ),
        r#"{"id":"gpt-5","provider":"openai"}"#,
        r#"{"inputTokens":2000,"outputTokens":250,"cacheReadTokens":1200,"cacheWriteTokens":0,"cost":0.01}"#,
    );
    write(
        "sessions/cli-beta/cli-beta.messages.json",
        &transcript(&[
            first_beta.clone(),
            cli(
                &format!(
                    r#""ts":{},"id":"beta-2""#,
                    instant("2026-01-02T11:00:00Z").millis()
                ),
                r#"{"id":"devstral-medium-latest","provider":"mistral"}"#,
                r#"{"inputTokens":500,"outputTokens":100,"cacheReadTokens":0,"cacheWriteTokens":0}"#,
            ),
            cli(
                &format!(
                    r#""ts":{},"id":"beta-2""#,
                    instant("2026-01-02T11:05:00Z").millis()
                ),
                &cli_sonnet,
                r#"{"inputTokens":1100,"outputTokens":20,"cacheReadTokens":1000,"cacheWriteTokens":0}"#,
            ),
        ]),
    );
    write(
        "data/sessions/cli-beta/cli-beta.messages.json",
        &transcript(&[first_beta]),
    );

    write(
        "tasks/task-stale/ui_messages.json",
        &document(&[request(
            instant("2025-12-27T22:00:00Z").millis(),
            &sonnet,
            r#"{"tokensIn":100,"tokensOut":50,"cacheReads":0,"cacheWrites":0}"#,
        )]),
    );

    let counted_if_discovered = document(&[request(
        instant("2025-12-31T13:00:00Z").millis(),
        &sonnet,
        ignored,
    )]);
    let transcript_if_discovered = transcript(&[cli(
        &format!(r#""ts":{}"#, instant("2025-12-31T13:01:00Z").millis()),
        &cli_sonnet,
        cli_ignored,
    )]);
    write("tasks/task-empty/ui_messages.json", "");
    write(
        "tasks/task-truncated/ui_messages.json",
        &counted_if_discovered[..counted_if_discovered.len() - 20],
    );
    write(
        "tasks/task-object/ui_messages.json",
        &transcript_if_discovered,
    );
    write(
        "sessions/cli-array/cli-array.messages.json",
        &counted_if_discovered,
    );
    write("sessions/cli-empty/cli-empty.messages.json", "");
    write(
        "tasks/nested/task-deep/ui_messages.json",
        &counted_if_discovered,
    );
    write("tasks/task-other/other.json", &counted_if_discovered);
    write(
        "sessions/cli-mismatch/another.messages.json",
        &transcript_if_discovered,
    );
    write("ui_messages.json", &counted_if_discovered);
    write("data/settings/ui_messages.json", &counted_if_discovered);

    let roots = vec![
        store.clone().into_os_string(),
        store.clone().into_os_string(),
        " ".into(),
        store.join("missing").into_os_string(),
    ];
    vec![("CLINE_DATA_DIR", roots)]
}
