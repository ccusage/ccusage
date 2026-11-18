use std::{
    io::Write,
    process::{Command, Stdio},
};

use ccusage_test_support::{Fixture, fs_fixture};
use serde_json::{Value, json};

#[test]
fn advertises_a_native_codex_statusline_resource_over_stdio() {
    let fixture = fs_fixture!({ "codex/sessions/.keep": "" });
    let replies = run_mcp(
        &fixture,
        &[
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion":"2025-06-18","capabilities":{},
                "clientInfo":{"name":"codex-test","version":"1"}
            }}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":"tools","method":"tools/list","params":{}}),
            json!({"jsonrpc":"2.0","id":3,"method":"ping"}),
        ],
    );

    assert_eq!(replies.len(), 3, "notifications must not produce replies");
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(
        replies[0]["result"]["capabilities"],
        json!({"tools":{},"resources":{}})
    );
    assert_eq!(replies[1]["id"], "tools");
    let tools = replies[1]["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "ccusage_codex_statusline");
    assert_eq!(tools[0]["_meta"]["ui"]["visibility"], json!(["app"]));
    assert_eq!(
        tools[0]["_meta"]["openai/ui"]["entrypoints"],
        json!([
            {"type":"statusline","resourceUri":"ui://ccusage/codex-statusline"}
        ])
    );
    assert_eq!(replies[2], json!({"jsonrpc":"2.0","id":3,"result":{}}));
}

#[test]
fn reports_protocol_errors_and_keeps_serving_valid_requests() {
    let fixture = fs_fixture!({ "codex/sessions/.keep": "" });
    let replies = run_mcp_raw(&fixture, &[
        "{broken-json}".to_string(),
        json!({"jsonrpc":"2.0","id":1,"method":"unknown"}).to_string(),
        json!({"jsonrpc":"2.0","id":2,"method":"resources/read","params":{"uri":"file:///private"}}).to_string(),
        read_resource(3, json!("not-a-thread-uuid")).to_string(),
        json!({"jsonrpc":"2.0","method":"notifications/cancelled"}).to_string(),
        json!({"jsonrpc":"2.0","id":"still-running","method":"ping"}).to_string(),
    ], &[]);
    assert_eq!(replies.len(), 5);
    for (reply, code) in replies.iter().zip([-32700, -32601, -32602, -32602]) {
        assert_eq!(reply["error"]["code"], code);
    }
    assert_eq!(replies[0]["id"], Value::Null);
    assert_eq!(
        replies[4],
        json!({"jsonrpc":"2.0","id":"still-running","result":{}})
    );
}

#[test]
fn rejects_unknown_tools_as_invalid_params() {
    let fixture = fs_fixture!({ "codex/sessions/.keep": "" });
    let replies = run_mcp(
        &fixture,
        &[
            json!({"jsonrpc":"2.0","id":"bad-tool","method":"tools/call","params":{"name":"unknown"}}),
        ],
    );
    assert_eq!(replies[0]["id"], "bad-tool");
    assert_eq!(replies[0]["error"]["code"], -32602);
}

#[test]
fn renders_the_displayed_thread_instead_of_other_sessions() {
    let fixture = fs_fixture!({
        "codex/sessions/rollout-2026-09-19T12-00-00-01a0bb8c-c7c0-7630-9d82-860b875930f0.jsonl": usage(100, 50),
        "codex/sessions/rollout-2026-09-20T12-00-00-01a0cc9d-d8d1-8741-ae93-971c986041a1.jsonl": usage(900, 100),
    });
    let replies = run_mcp(
        &fixture,
        &[
            read_resource(1, json!("01a0bb8c-c7c0-7630-9d82-860b875930f0")),
            read_resource(2, json!("01a0cc9d-d8d1-8741-ae93-971c986041a1")),
            read_resource(3, json!("01a0bb8c-c7c0-7630-9d82-000000000000")),
            read_resource(4, Value::Null),
        ],
    );

    for (reply, text) in replies.iter().zip([
        json!("Codex | Session: 150 tokens | Est. $0.00"),
        json!("Codex | Session: 1,000 tokens | Est. $0.00"),
        json!("Codex | Session: 0 tokens | Est. $0.00"),
        Value::Null,
    ]) {
        let contents = reply["result"]["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["uri"], "ui://ccusage/codex-statusline");
        assert_eq!(contents[0]["mimeType"], "application/json");
        let tree: Value = serde_json::from_str(contents[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(tree, text);
    }
}

#[test]
fn native_resources_apply_pricing_overrides_and_ignore_preview_filters() {
    let fixture = fs_fixture!({
        "codex/sessions/rollout-2026-09-19T12-00-00-01a0bb8c-c7c0-7630-9d82-860b875930f0.jsonl": usage(100, 50),
        "ccusage.json": r#"{"codex":{"commands":{"statusline":{
            "since":"20990101","json":true,
            "pricingOverrides":{"gpt-5":{"inputCostPerToken":0.01,"outputCostPerToken":0.02}}
        }}}}"#,
    });
    let request = [read_resource(1, json!("01a0bb8c-c7c0-7630-9d82-860b875930f0")).to_string()];
    let config = fixture.path("ccusage.json");
    let args = ["--config", config.to_str().unwrap()];
    let replies = run_mcp_raw(&fixture, &request, &args);
    let text: String = serde_json::from_str(
        replies[0]["result"]["contents"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(text, "Codex | Session: 150 tokens | Est. $2.00");

    let replies = run_mcp_raw(&fixture, &request, &[&args[..], &["--no-cost"]].concat());
    let text: String = serde_json::from_str(
        replies[0]["result"]["contents"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(text, "Codex | Session: 150 tokens");
}

fn read_resource(id: u64, thread: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"resources/read","params":{
        "uri":"ui://ccusage/codex-statusline",
        "_meta":{"com.openai/codex-statusline-state":{"thread":{"id":thread}}}
    }})
}

fn usage(input: u64, output: u64) -> String {
    json!({"timestamp":"2026-09-19T12:00:00Z","type":"event_msg","payload":{
        "type":"token_count","info":{"model":"gpt-5","last_token_usage":{
            "input_tokens":input,"output_tokens":output,"total_tokens":input+output
        }}
    }})
    .to_string()
}

fn run_mcp(fixture: &Fixture, requests: &[Value]) -> Vec<Value> {
    run_mcp_raw(
        fixture,
        &requests.iter().map(Value::to_string).collect::<Vec<_>>(),
        &[],
    )
}

fn run_mcp_raw(fixture: &Fixture, requests: &[String], args: &[&str]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ccusage"))
        .args([
            "codex",
            "statusline",
            "--mcp",
            "--offline",
            "--timezone",
            "UTC",
        ])
        .args(args)
        .env("CODEX_HOME", fixture.path("codex"))
        .env("HOME", fixture.path("empty-home"))
        .env("USERPROFILE", fixture.path("empty-userprofile"))
        .env("XDG_CONFIG_HOME", fixture.path("empty-config"))
        .env_remove("LOG_LEVEL")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for request in requests {
        writeln!(stdin, "{request}").unwrap();
    }
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).expect("stdout must contain only JSON-RPC"))
        .collect()
}
