#![cfg(test)]

mod support;

use std::{
    fs,
    io::{self, BufRead, BufReader, Write},
    process::{Child, ChildStdin, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use ccusage_core::{MILLIS_PER_DAY, TimestampMs, format_rfc3339_millis, utc_now};
use ccusage_test_support::{Fixture, fs_fixture};
use serde_json::{Value, json};

const THREAD_UUID: &str = "01a0bb8c-c7c0-7630-9d82-860b875930f0";
const OTHER_THREAD_UUID: &str = "01a0cc9d-d8d1-8741-ae93-971c986041a1";
const REVERT_UUID: &str = "01a0ddae-e9e2-9852-bf04-a21d097152b2";
const LATE_REVERT_UUID: &str = "01a0eebf-fa03-a963-c015-b32e208263c3";
const LOADING_TEXT: &str = "Codex | Loading usage...";

#[test]
fn native_resource_keeps_valid_usage_when_another_rollout_is_corrupt() {
    let fixture = native_usage_fixture();
    fixture.write_file("codex/sessions/bad.jsonl.zst", "corrupt-rollout-canary");
    let mut process = McpProcess::start(&fixture, &["--no-cost"]);
    assert_eq!(
        process.read_resource_line(1, THREAD_UUID),
        "Codex | Session: 600 tokens | Today: 300 tokens"
    );
    process.finish();
}

#[test]
fn native_fork_parent_warning_stays_off_stdout() {
    let fixture = fs_fixture!({
        "codex/sessions/unrelated.jsonl.zst": "corrupt rollout",
    });
    fixture.write_file(
        format!("codex/sessions/rollout-2000-01-01T12-00-00-{THREAD_UUID}.jsonl"),
        [
            json!({
                "type":"session_meta",
                "timestamp":"2000-01-01T12:00:00Z",
                "payload":{"id":THREAD_UUID,"forked_from_id":OTHER_THREAD_UUID}
            })
            .to_string(),
            usage("2000-01-01T11:59:00Z", 100, 50),
            usage("2000-01-01T12:01:00Z", 100, 50),
        ]
        .join("\n"),
    );
    fixture.write_file(
        format!("codex/sessions/rollout-2000-01-01T11-00-00-{OTHER_THREAD_UUID}.jsonl.zst"),
        "corrupt parent",
    );
    let stderr = fixture.path("stderr.log");
    let mut process = McpProcess::start_with_stderr(
        &fixture,
        &["--no-cost"],
        fs::File::create(&stderr).unwrap().into(),
    );

    assert_eq!(
        process.read_resource_line(1, THREAD_UUID),
        "Codex | Session: 300 tokens | Today: 0 tokens"
    );
    process.finish();
    assert!(matches!(
        process.stdout.recv_timeout(Duration::from_secs(5)),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
    let stderr = fs::read_to_string(stderr).unwrap();
    assert_eq!(
        stderr.lines().filter(|line| *line == "WARN  codex: could not read the parent rollout for 1 forked session. Replayed usage may be counted twice.").count(),
        1,
        "{stderr}"
    );
}

#[test]
fn native_resource_emits_warnings_when_they_change() {
    let fixture = native_usage_fixture();
    let corrupt = fixture.write_file("codex/sessions/bad.jsonl.zst", "corrupt rollout");
    let stderr = fixture.path("stderr.log");
    let mut process = McpProcess::start_with_stderr(
        &fixture,
        &["--no-cost"],
        fs::File::create(&stderr).unwrap().into(),
    );

    process.read_resource_line(1, THREAD_UUID);
    process.read_resource_line(2, OTHER_THREAD_UUID);
    let first_warnings = fs::read_to_string(&stderr).unwrap();
    assert_eq!(
        first_warnings.matches("WARN  codex: skipped ").count(),
        1,
        "{first_warnings}"
    );

    fs::remove_file(&corrupt).unwrap();
    process.read_resource_line(3, REVERT_UUID);
    fs::write(corrupt, "corrupt rollout").unwrap();
    process.read_resource_line(4, LATE_REVERT_UUID);
    process.finish();

    let stderr = fs::read_to_string(stderr).unwrap();
    assert_eq!(
        stderr.matches("WARN  codex: skipped ").count(),
        2,
        "{stderr}"
    );
}

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
fn native_resources_show_logical_session_and_global_today_usage() {
    let fixture = native_usage_fixture();
    let config = fixture.write_file(
        "ccusage.json",
        r#"{"codex":{"commands":{"statusline":{
            "pricingOverrides":{"gpt-5":{"inputCostPerToken":0.01,"outputCostPerToken":0.02}}
        }}}}"#,
    );
    let mut process = McpProcess::start(
        &fixture,
        &[
            "--config",
            config.to_str().unwrap(),
            "--since",
            "20990101",
            "--until",
            "20990101",
        ],
    );

    assert_eq!(
        process.read_resource_line(1, THREAD_UUID),
        "Codex | Est. $8.00 session / $4.00 today | 600 tokens"
    );
    assert_eq!(
        process.read_resource_line(2, OTHER_THREAD_UUID),
        "Codex | Est. $6.00 session / $4.00 today | 450 tokens"
    );
    assert_eq!(
        process.read_resource_value(read_resource(3, Value::Null)),
        Value::Null
    );
    process.finish();
}

#[test]
fn native_resource_keeps_ready_lines_for_alternating_threads() {
    let fixture = native_usage_fixture();
    let mut process = McpProcess::start(&fixture, &["--no-cost"]);

    assert_eq!(
        process.read_resource_line(1, THREAD_UUID),
        "Codex | Session: 600 tokens | Today: 300 tokens"
    );
    assert_eq!(
        process.read_resource_line(2, OTHER_THREAD_UUID),
        "Codex | Session: 450 tokens | Today: 300 tokens"
    );
    assert_eq!(
        process.read_resource_value(read_resource(3, json!(THREAD_UUID))),
        json!("Codex | Session: 600 tokens | Today: 300 tokens")
    );
    process.finish();
}

#[test]
fn native_resource_refreshes_cached_usage_after_a_new_revert_rollout() {
    let fixture = native_usage_fixture();
    let mut process = McpProcess::start(&fixture, &["--no-cost"]);
    assert_eq!(
        process.read_resource_line(1, THREAD_UUID),
        "Codex | Session: 600 tokens | Today: 300 tokens"
    );

    let now = utc_now();
    let date_start = now.as_millis().div_euclid(MILLIS_PER_DAY) * MILLIS_PER_DAY;
    let timestamp = TimestampMs::from_millis(date_start + 14 * 60 * 60 * 1_000);
    let timestamp_text = format_rfc3339_millis(timestamp);
    let file_stamp = rollout_stamp(&timestamp_text);
    let new_usage = usage(&timestamp_text, 200, 100);
    fixture.write_file(
        format!("codex/sessions/rollout-{file_stamp}-{THREAD_UUID}_{LATE_REVERT_UUID}.jsonl"),
        rollout(THREAD_UUID, &timestamp_text, &[new_usage]),
    );

    assert_eq!(
        process.read_resource_line_until(
            2,
            THREAD_UUID,
            "Codex | Session: 900 tokens | Today: 600 tokens",
        ),
        "Codex | Session: 900 tokens | Today: 600 tokens"
    );
    process.finish();
}

#[test]
fn native_resource_refreshes_cached_cost_after_the_configured_tier_changes() {
    let timestamp = "2000-01-01T12:00:00Z";
    let event = json!({
        "timestamp": timestamp,
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": {
                "model": "gpt-5.6-sol",
                "last_token_usage": { "input_tokens": 10_000, "total_tokens": 10_000 },
            },
        },
    })
    .to_string();
    let fixture = fs_fixture!({ "codex/config.toml": "service_tier = 'standard'" });
    fixture.write_file(
        format!("codex/sessions/rollout-2000-01-01T12-00-00-{THREAD_UUID}.jsonl"),
        rollout(THREAD_UUID, timestamp, &[event]),
    );
    let mut process = McpProcess::start(&fixture, &[]);
    assert_eq!(
        process.read_resource_line(1, THREAD_UUID),
        "Codex | Est. $0.04 session / $0.00 today | 10,000 tokens"
    );

    fixture.write_file("codex/config.toml", "service_tier = 'fast'");
    process.read_resource_line_until(
        2,
        THREAD_UUID,
        "Codex | Est. $0.08 session / $0.00 today | 10,000 tokens",
    );
    process.finish();
}

#[test]
fn native_resources_support_no_cost_and_empty_usage() {
    let fixture = native_usage_fixture();
    let mut process = McpProcess::start(&fixture, &["--no-cost"]);
    assert_eq!(
        process.read_resource_line(1, THREAD_UUID),
        "Codex | Session: 600 tokens | Today: 300 tokens"
    );
    process.finish();

    let empty = fs_fixture!({ "codex/sessions/.keep": "" });
    let mut process = McpProcess::start(&empty, &[]);
    assert_eq!(
        process.read_resource_line(1, THREAD_UUID),
        "Codex | Est. $0.00 session / $0.00 today | 0 tokens"
    );
    process.finish();

    let mut process = McpProcess::start(&empty, &["--no-cost"]);
    assert_eq!(
        process.read_resource_line(1, THREAD_UUID),
        "Codex | Session: 0 tokens | Today: 0 tokens"
    );
    process.finish();
}

#[test]
fn native_resource_prefixes_the_sanitized_model_and_reasoning_level() {
    let fixture = native_usage_fixture();
    let mut process = McpProcess::start(&fixture, &["--no-cost"]);
    assert_eq!(
        process.read_resource_line_with_model(
            1,
            THREAD_UUID,
            json!({"display_name":"GPT-5\ninjected","reasoning_level":"high"}),
        ),
        "🤖 GPT-5\\ninjected (high) | Session: 600 tokens | Today: 300 tokens"
    );
    process.finish();
}

fn read_resource(id: u64, thread: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"resources/read","params":{
        "uri":"ui://ccusage/codex-statusline",
        "_meta":{"com.openai/codex-statusline-state":{"thread":{"id":thread}}}
    }})
}

fn read_resource_with_model(id: u64, thread: Value, model: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"resources/read","params":{
        "uri":"ui://ccusage/codex-statusline",
        "_meta":{"com.openai/codex-statusline-state":{"thread":{"id":thread,"model":model}}}
    }})
}

fn usage(timestamp: &str, input: u64, output: u64) -> String {
    json!({"timestamp":timestamp,"type":"event_msg","payload":{
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
    let mut child = support::hermetic_command(fixture.root())
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
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start the MCP server");
    let mut stdin = child.stdin.take().expect("piped MCP stdin");
    for request in requests {
        writeln!(stdin, "{request}").expect("write MCP request");
    }
    drop(stdin);
    let output = child.wait_with_output().expect("wait for MCP output");
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
        .expect("MCP stdout is UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("stdout must contain only JSON-RPC"))
        .collect()
}

#[test]
#[should_panic(expected = "MCP server did not reply within 5 seconds: Timeout")]
fn stops_waiting_when_the_provider_sends_no_reply() {
    let fixture = fs_fixture!({ "codex/sessions/.keep": "" });
    let mut process = McpProcess::start(&fixture, &[]);
    process.read_resource_value(json!({
        "jsonrpc":"2.0","method":"notifications/initialized"
    }));
}

struct McpProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: mpsc::Receiver<io::Result<String>>,
}

impl McpProcess {
    fn start(fixture: &Fixture, args: &[&str]) -> Self {
        Self::start_with_stderr(fixture, args, Stdio::null())
    }

    fn start_with_stderr(fixture: &Fixture, args: &[&str], stderr: Stdio) -> Self {
        let mut child = support::hermetic_command(fixture.root())
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
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()
            .expect("start the streaming MCP server");
        let stdin = child.stdin.take().expect("piped MCP stdin");
        let stream = child.stdout.take().expect("piped MCP stdout");
        let (sender, stdout) = mpsc::channel();
        ccusage_core::parallel::spawn_worker(move || {
            for line in BufReader::new(stream).lines() {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin: Some(stdin),
            stdout,
        }
    }

    fn read_resource_line(&mut self, request_id: u64, thread_id: &str) -> String {
        self.read_resource_line_with(request_id, thread_id, None)
    }

    fn read_resource_line_until(
        &mut self,
        request_id: u64,
        thread_id: &str,
        expected: &str,
    ) -> String {
        let started = Instant::now();
        loop {
            let value = self.read_resource_value(read_resource(request_id, json!(thread_id)));
            let line = value
                .as_str()
                .unwrap_or_else(|| panic!("expected a statusline string, got {value}"));
            if line == expected {
                return line.to_string();
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "Codex statusline did not reach expected output {expected:?}; latest output: {line:?}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn read_resource_line_with_model(
        &mut self,
        request_id: u64,
        thread_id: &str,
        model: Value,
    ) -> String {
        self.read_resource_line_with(request_id, thread_id, Some(model))
    }

    fn read_resource_line_with(
        &mut self,
        request_id: u64,
        thread_id: &str,
        model: Option<Value>,
    ) -> String {
        let started = Instant::now();
        loop {
            let request = match &model {
                Some(model) => {
                    read_resource_with_model(request_id, json!(thread_id), model.clone())
                }
                None => read_resource(request_id, json!(thread_id)),
            };
            let value = self.read_resource_value(request);
            let line = value
                .as_str()
                .unwrap_or_else(|| panic!("expected a statusline string, got {value}"));
            if line != LOADING_TEXT {
                return line.to_string();
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "Codex statusline remained in the loading state"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn read_resource_value(&mut self, request: Value) -> Value {
        let stdin = self.stdin.as_mut().expect("MCP stdin is still open");
        writeln!(stdin, "{request}").expect("write MCP request");
        let line = self
            .stdout
            .recv_timeout(Duration::from_secs(5))
            .expect("MCP server did not reply within 5 seconds")
            .expect("MCP stdout could not be read");
        let reply: Value = serde_json::from_str(&line).expect("stdout must contain JSON-RPC");
        assert!(
            reply.get("error").is_none(),
            "unexpected MCP error: {reply}"
        );
        let text = reply["result"]["contents"][0]["text"]
            .as_str()
            .expect("resource read must return a text content block");
        serde_json::from_str(text).expect("resource text must contain JSON")
    }

    fn finish(&mut self) {
        self.stdin.take();
        let started = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().expect("poll MCP status") {
                assert!(status.success(), "MCP server exited with {status}");
                return;
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "MCP server did not exit within 5 seconds"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for McpProcess {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn native_usage_fixture() -> Fixture {
    let fixture = fs_fixture!({ "codex/sessions/.keep": "" });
    let now = utc_now();
    let base = TimestampMs::from_millis(
        now.as_millis().div_euclid(MILLIS_PER_DAY) * MILLIS_PER_DAY + 12 * 60 * 60 * 1_000,
    );
    let yesterday = base
        .checked_add_millis(-MILLIS_PER_DAY)
        .expect("yesterday timestamp is in range");
    let today = base;
    let tomorrow = base
        .checked_add_millis(MILLIS_PER_DAY)
        .expect("tomorrow timestamp is in range");
    let reverted = yesterday
        .checked_add_millis(1_000)
        .expect("reverted timestamp is in range");

    for timestamp in [yesterday, today, tomorrow] {
        let date_start = timestamp.as_millis().div_euclid(MILLIS_PER_DAY) * MILLIS_PER_DAY;
        for (thread_id, hour) in [(THREAD_UUID, 12), (OTHER_THREAD_UUID, 13)] {
            let timestamp = TimestampMs::from_millis(date_start + hour * 60 * 60 * 1_000);
            let timestamp_text = format_rfc3339_millis(timestamp);
            let file_stamp = rollout_stamp(&timestamp_text);
            let event = usage(&timestamp_text, 100, 50);
            let path = format!("codex/sessions/rollout-{file_stamp}-{thread_id}.jsonl");
            fixture.write_file(path, rollout(thread_id, &timestamp_text, &[event]));
        }
    }

    let reverted_text = format_rfc3339_millis(reverted);
    let file_stamp = rollout_stamp(&reverted_text);
    let new_usage = usage(&reverted_text, 100, 50);
    fixture.write_file(
        format!("codex/sessions/rollout-{file_stamp}-{THREAD_UUID}_{REVERT_UUID}.jsonl"),
        rollout(THREAD_UUID, &reverted_text, &[new_usage]),
    );
    fixture
}

fn rollout(thread_id: &str, timestamp: &str, usage_events: &[String]) -> String {
    let mut lines = vec![
        json!({"timestamp":timestamp,"type":"session_meta","payload":{"id":thread_id}}).to_string(),
    ];
    lines.extend(usage_events.iter().cloned());
    lines.join("\n")
}

fn rollout_stamp(timestamp: &str) -> String {
    timestamp
        .replace(':', "-")
        .strip_suffix(".000Z")
        .unwrap_or(timestamp)
        .to_string()
}
