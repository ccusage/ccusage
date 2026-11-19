use std::ffi::OsString;
use std::path::{Path, PathBuf};

use ccusage_test_support::{instant, set_modified};

const SONNET: &str = "claude-sonnet-4-20250514";
const OPUS: &str = "claude-opus-4-20250514";
const HAIKU: &str = "claude-haiku-4-5-20251001";
const GPT: &str = "gpt-5";
const UNPRICED: &str = "copilot-mystery-1";

const CREDIT: u64 = 1_000_000_000;

fn checkpoint(id: &str, timestamp: &str, credits: u64) -> String {
    format!(
        r#"{{"type":"session.usage_checkpoint","id":"{id}","timestamp":"{timestamp}","data":{{"totalNanoAiu":{},"promptCacheBreakState":[]}}}}"#,
        credits * CREDIT
    )
}

fn shutdown(head: &str, total: &str, models: &str) -> String {
    format!(r#"{{"type":"session.shutdown",{head}"data":{{{total}"modelMetrics":{{{models}}}}}}}"#)
}

fn usage_record(head: &str, model: &str, usage: &str) -> String {
    format!(
        r#"{{"type":"session.usage_record",{head}"data":{{"usage":{{"model":"{model}","interactionType":"conversation-agent",{usage}}}}}}}"#
    )
}

fn model(name: &str, usage: &str, rest: &str) -> String {
    format!(r#""{name}":{{"usage":{{{usage}}}{rest}}}"#)
}

fn chat_span((trace_id, span_id): (&str, &str), end_seconds: i64, attributes: &str) -> String {
    format!(
        r#"{{"type":"span","traceId":"{trace_id}","spanId":"{span_id}","name":"chat","endTime":[{end_seconds},0],"attributes":{{"gen_ai.operation.name":"chat"{attributes}}}}}"#
    )
}

fn call(session: &str, model: &str, usage: &str) -> String {
    format!(r#","gen_ai.conversation.id":"{session}","gen_ai.response.model":"{model}",{usage}"#)
}

fn tokens(input: u64, output: u64) -> String {
    format!(r#""gen_ai.usage.input_tokens":{input},"gen_ai.usage.output_tokens":{output}"#)
}

fn seed_shutdowns_with_credit_checkpoints(write: &impl Fn(&str, &[String]) -> PathBuf) {
    let first_shutdown = shutdown(
        r#""id":"sd-a1","timestamp":"2025-12-30T10:00:00.000Z","#,
        &format!(r#""totalNanoAiu":{},"#, 30 * CREDIT),
        &[
            model(
                &format!("{SONNET}-1m"),
                r#""inputTokens":1000,"outputTokens":200,"cacheReadTokens":400,"cacheWriteTokens":100,"reasoningTokens":50"#,
                &format!(
                    r#","requests":{{"count":2,"cost":999}},"totalNanoAiu":{}"#,
                    20 * CREDIT
                ),
            ),
            model("empty-model", r#""inputTokens":0,"outputTokens":0"#, ""),
            r#""no-usage-model":{"requests":{"count":9}}"#.to_string(),
        ]
        .join(","),
    );
    write(
        "session-state/session-alpha/events.jsonl",
        &[
            "not json".to_string(),
            r#"{"type":"session.start","id":"start-a","timestamp":"2025-12-30T08:59:00.000Z","data":{"copilotVersion":"1.0.0"}}"#.to_string(),
            format!(
                r#"{{"type":"tool.execution_complete","timestamp":"2025-12-30T09:30:00.000Z","data":{{"message":"session.shutdown","modelMetrics":{{{}}}}}}}"#,
                model(SONNET, r#""inputTokens":9999,"outputTokens":9999"#, "")
            ),
            checkpoint("cp-a1", "2025-12-30T09:00:00.000Z", 20),
            first_shutdown.clone(),
            first_shutdown,
            shutdown(
                r#""id":"sd-no-timestamp","#,
                "",
                &model(SONNET, r#""inputTokens":9999,"outputTokens":9999"#, ""),
            ),
            shutdown(
                r#""id":"sd-bad-timestamp","timestamp":"not-a-time","#,
                "",
                &model(SONNET, r#""inputTokens":8888,"outputTokens":8888"#, ""),
            ),
            shutdown(
                r#""id":"sd-a2","timestamp":"2025-12-31T16:00:00.000Z","#,
                &format!(r#""totalNanoAiu":{},"#, 55 * CREDIT),
                &[
                    model(
                        &format!("{SONNET}-1m-internal"),
                        r#""inputTokens":3000,"outputTokens":700,"cacheReadTokens":1200,"cacheWriteTokens":300,"reasoningTokens":90"#,
                        &format!(
                            r#","requests":{{"count":5}},"totalNanoAiu":{}"#,
                            35 * CREDIT
                        ),
                    ),
                    model(
                        UNPRICED,
                        r#""inputTokens":300,"outputTokens":70"#,
                        &format!(
                            r#","requests":{{"count":1}},"totalNanoAiu":{}"#,
                            5 * CREDIT
                        ),
                    ),
                ]
                .join(","),
            ),
            checkpoint("cp-a2", "2026-01-01T12:00:00.000Z", 70),
            checkpoint("cp-a3", "2026-01-02T08:00:00.000Z", 80),
        ],
    );
    write(
        "session-state/session-alpha/other.jsonl",
        &[shutdown(
            r#""id":"sd-other","timestamp":"2025-12-31T12:00:00.000Z","#,
            "",
            &model(SONNET, r#""inputTokens":9999,"outputTokens":9999"#, ""),
        )],
    );
}

fn seed_replayed_shutdown_with_otel_coverage(write: &impl Fn(&str, &[String]) -> PathBuf) {
    let shutdown_without_id = shutdown(
        r#""timestamp":"2025-12-31T10:00:00.000Z","#,
        "",
        &model(
            GPT,
            r#""inputTokens":2000,"outputTokens":300,"cacheReadTokens":500"#,
            r#","requests":{"count":2}"#,
        ),
    );
    write(
        "session-state/session-beta/events.jsonl",
        &[
            checkpoint("cp-b1", "2025-12-31T08:00:00.000Z", 7_777),
            shutdown_without_id.clone(),
            shutdown_without_id,
            shutdown(
                r#""id":"sd-b2","timestamp":"2026-01-02T09:00:00.000Z","#,
                "",
                &model(
                    GPT,
                    r#""inputTokens":3000,"outputTokens":420,"cacheReadTokens":900"#,
                    r#","requests":{"count":3}"#,
                ),
            ),
        ],
    );
}

fn seed_incomplete_shutdown_covered_by_otel(write: &impl Fn(&str, &[String]) -> PathBuf) {
    write(
        "session-state/session-gamma/events.jsonl",
        &[
            shutdown(
                r#""id":"sd-g1","timestamp":"2025-12-30T12:00:00.000Z","#,
                "",
                &model(
                    HAIKU,
                    r#""inputTokens":100,"outputTokens":10"#,
                    r#","requests":{"count":1}"#,
                ),
            ),
            shutdown(
                r#""id":"sd-g2","timestamp":"2026-01-01T12:00:00.000Z","#,
                "",
                &model(
                    HAIKU,
                    r#""inputTokens":7877,"outputTokens":787"#,
                    r#","requests":{"count":2}"#,
                ),
            ),
        ],
    );
}

fn seed_shutdown_without_credit_checkpoints(write: &impl Fn(&str, &[String]) -> PathBuf) {
    write(
        "session-state/session-delta/events.jsonl",
        &[shutdown(
            r#""id":"sd-d1","timestamp":"2025-12-27T22:00:00.000Z","#,
            &format!(r#""totalNanoAiu":{},"#, 60 * CREDIT),
            &model(
                OPUS,
                r#""inputTokens":600,"outputTokens":150,"cacheReadTokens":200,"cacheWriteTokens":100,"reasoningTokens":40"#,
                &format!(
                    r#","requests":{{"count":4}},"totalNanoAiu":{}"#,
                    40 * CREDIT
                ),
            ),
        )],
    );
}

fn seed_checkpoints_without_model_credit_total(write: &impl Fn(&str, &[String]) -> PathBuf) {
    write(
        "session-state/session-epsilon/events.jsonl",
        &[
            checkpoint("cp-e1", "2026-01-02T10:00:00.000Z", 8_888),
            shutdown(
                r#""id":"sd-e1","timestamp":"2026-01-02T11:00:00.000Z","#,
                &format!(r#""totalNanoAiu":{},"#, 8_888 * CREDIT),
                &model(
                    GPT,
                    r#""inputTokens":450,"outputTokens":50"#,
                    r#","requests":{"count":1}"#,
                ),
            ),
            checkpoint("cp-e2", "2026-01-02T12:00:00.000Z", 9_999),
        ],
    );
}

fn seed_checkpoints_without_shutdown(write: &impl Fn(&str, &[String]) -> PathBuf) {
    let only_checkpoint = checkpoint("cp-z1", "2026-01-01T03:00:00.000Z", 12);
    write(
        "session-state/session-zeta/events.jsonl",
        &[
            only_checkpoint.clone(),
            only_checkpoint,
            r#"{"type":"session.usage_checkpoint","id":"cp-z2","timestamp":"2026-01-01T04:00:00.000Z","data":{}}"#.to_string(),
            r#"{"type":"session.usage_checkpoint","id":"cp-z3","timestamp":"2026-01-01T05:00:00.000Z","data":{"totalNanoAiu":-5000000000}}"#.to_string(),
        ],
    );
}

fn seed_per_call_usage_with_accounted_and_unaccounted_replays(
    write: &impl Fn(&str, &[String]) -> PathBuf,
) {
    let accounted_record = usage_record(
        r#""id":"ur-h1","timestamp":"2025-12-31T11:00:00.000Z","#,
        SONNET,
        r#""inputTokens":1500,"outputTokens":90,"cacheReadTokens":1000,"cacheWriteTokens":100,"accounting":{"sourceSessionId":"session-eta","sequence":1,"usageId":"call-h1"},"apiCallId":"chatcmpl-h1","serviceRequestId":"svc-h1","copilotUsage":{"totalNanoAiu":3000000000}"#,
    );
    let unaccounted_record = usage_record(
        r#""id":"ur-h3","timestamp":"2026-01-02T09:30:00.000Z","#,
        OPUS,
        r#""inputTokens":200,"outputTokens":20"#,
    );
    write(
        "session-state/session-eta/events.jsonl",
        &[
            r#"{"type":"session.start","id":"start-h","timestamp":"2025-12-31T10:59:00.000Z","data":{"sessionId":"session-eta"}}"#.to_string(),
            accounted_record.clone(),
            usage_record(
                r#""id":"ur-h2","timestamp":"2026-01-01T15:30:00.000Z","#,
                GPT,
                r#""inputTokens":600,"outputTokens":60,"cacheReadTokens":200,"accounting":{"sourceSessionId":"session-eta","sequence":2,"usageId":"call-h2"},"apiCallId":"chatcmpl-h2","serviceRequestId":"svc-h2""#,
            ),
            unaccounted_record.clone(),
            unaccounted_record,
            usage_record(
                r#""id":"ur-h4","timestamp":"2026-01-02T09:40:00.000Z","#,
                "",
                r#""inputTokens":9999,"outputTokens":9999"#,
            ),
            r#"{"type":"session.usage_checkpoint","id":"cp-h1","timestamp":"2026-01-02T09:40:01.000Z","data":{"totalNanoAiu":3000000000,"usageAccountingWatermarks":{"session-eta":2}}}"#.to_string(),
        ],
    );
    write(
        "session-state/session-theta/events.jsonl",
        &[accounted_record],
    );
}

pub(crate) fn seed(root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
    let home = root.join("copilot");
    let write = |relative: &str, lines: &[String]| {
        let path = home.join(relative);
        std::fs::create_dir_all(path.parent().expect("fixture file has a parent"))
            .expect("failed to create Copilot fixture directory");
        std::fs::write(&path, lines.join("\n")).expect("failed to write Copilot fixture file");
        path
    };

    seed_shutdowns_with_credit_checkpoints(&write);

    seed_replayed_shutdown_with_otel_coverage(&write);

    seed_incomplete_shutdown_covered_by_otel(&write);

    seed_shutdown_without_credit_checkpoints(&write);

    seed_checkpoints_without_model_credit_total(&write);

    seed_checkpoints_without_shutdown(&write);

    seed_per_call_usage_with_accounted_and_unaccounted_replays(&write);

    write("session-state/session-empty/events.jsonl", &[]);
    write(
        "session-state/session-truncated/events.jsonl",
        &[format!(
            r#"{{"type":"session.shutdown","id":"sd-t1","timestamp":"2025-12-31T12:00:00.000Z","data":{{"modelMetrics":{{"{SONNET}":{{"usage":{{"inputTokens":9999,"outp"#
        )],
    );
    let undiscovered = [shutdown(
        r#""id":"sd-x1","timestamp":"2025-12-31T12:00:00.000Z","#,
        "",
        &model(SONNET, r#""inputTokens":9999,"outputTokens":9999"#, ""),
    )];
    write("session-state/nested/session-x/events.jsonl", &undiscovered);
    write("session-state/events.jsonl", &undiscovered);

    let exported_twice = chat_span(
        ("trace-export", "span-x1"),
        instant("2026-01-02T15:00:00Z").secs(),
        &format!(
            r#"{},"gen_ai.response.id":"resp-x1""#,
            call("conv-export", OPUS, &tokens(900, 90))
        ),
    );
    let exporter = write(
        "exporter/explicit.jsonl",
        std::slice::from_ref(&exported_twice),
    );

    write(
        "otel/cli/trace.jsonl",
        &[
            r#"{"type":"metric","name":"gen_ai.client.token.usage","attributes":{"gen_ai.usage.input_tokens":9999,"gen_ai.usage.output_tokens":9999}}"#.to_string(),
            r#"{"type":"span","traceId":"trace-bare","spanId":"span-bare","name":"chat"}"#.to_string(),
            r#"{"type":"span","traceId":"trace-cut","spanId":"span-cut","name":"chat","attributes":{"gen_ai.operation.name":"chat","gen_ai.usage.input_tokens":9999,"#.to_string(),
            chat_span(
                ("trace-b1", "span-b1"),
                instant("2025-12-31T09:00:00Z").secs(),
                &call("session-beta", GPT, &tokens(9999, 9999)),
            ),
            chat_span(
                ("trace-b1", "span-b2"),
                instant("2025-12-31T09:30:00Z").secs(),
                &call("session-beta", GPT, &tokens(7777, 7777)),
            ),
            chat_span(
                ("trace-b3", "span-b3"),
                instant("2025-12-31T23:30:00Z").secs(),
                &call(
                    "session-beta",
                    GPT,
                    &format!(
                        r#"{},"gen_ai.usage.cache_read.input_tokens":300"#,
                        tokens(800, 120)
                    ),
                ),
            ),
            format!(
                r#"{{"type":"span","traceId":"trace-b4","spanId":"span-b4","name":"chat","time":[{},0],"attributes":{{"gen_ai.operation.name":"chat"{}}}}}"#,
                instant("2026-01-02T10:00:00Z").secs(),
                call("session-beta", GPT, &tokens(90, 9))
            ),
            chat_span(
                ("trace-b1", "span-b5"),
                instant("2025-12-31T09:15:00Z").secs(),
                &call("session-beta", HAIKU, &tokens(400, 60)),
            ),
            chat_span(
                ("trace-g1", "span-g1"),
                instant("2025-12-31T15:30:00Z").secs(),
                &call(
                    "session-gamma",
                    &format!("{HAIKU}-1m"),
                    &format!(
                        r#"{},"gen_ai.usage.cache_read.input_tokens":100"#,
                        tokens(210, 30)
                    ),
                ),
            ),
            format!(
                r#"{{"type":"span","traceId":"trace-g2","spanId":"span-g2","name":"chat","timestamp":{},"attributes":{{"gen_ai.operation.name":"chat"{}}}}}"#,
                (instant("2025-12-31T20:00:00Z").secs()) * 1_000_000_000,
                call("session-gamma", HAIKU, &tokens(220, 31))
            ),
            chat_span(
                ("trace-g3", "span-g3"),
                instant("2026-01-01T11:00:00Z").secs(),
                &call("session-gamma", HAIKU, &tokens(230, 32)),
            ),
            chat_span(
                ("trace-h1", "span-h1"),
                instant("2025-12-31T11:00:00Z").secs(),
                &format!(
                    r#"{},"github.copilot.service_request_id":"svc-h1""#,
                    call("session-eta", SONNET, &tokens(1500, 90))
                ),
            ),
            chat_span(
                ("trace-h2", "span-h2"),
                instant("2026-01-01T15:30:00Z").secs(),
                &format!(
                    r#"{},"gen_ai.response.id":"chatcmpl-h2""#,
                    call("session-eta", GPT, &tokens(600, 60))
                ),
            ),
            chat_span(
                ("trace-h2", "span-h2b"),
                instant("2026-01-01T15:31:00Z").secs(),
                &format!(
                    r#"{},"gen_ai.response.id":"chatcmpl-h2""#,
                    call("session-eta", HAIKU, &tokens(50, 5))
                ),
            ),
            exported_twice,
        ],
    );

    let first_call = format!(
        r#"{},"gen_ai.response.id":"resp-1","gen_ai.usage.cache_read.input_tokens":200,"gen_ai.usage.cache_creation.input_tokens":100,"gen_ai.usage.reasoning.output_tokens":20,"gen_ai.usage.total_tokens":1170"#,
        call("conv-vscode", SONNET, &tokens(1000, 150))
    );
    write(
        "otel/vscode.jsonl",
        &[
            chat_span(
                ("trace-1", "span-v1"),
                instant("2025-12-30T14:00:00Z").secs(),
                &first_call,
            ),
            format!(
                r#"{{"hrTime":[{},0],"traceId":"trace-1","spanId":"span-v1","_body":"GenAI inference: {SONNET}","attributes":{{"event.name":"gen_ai.client.inference.operation.details","gen_ai.response.model":"{SONNET}",{}}}}}"#,
                instant("2025-12-30T14:00:00Z").secs(),
                tokens(9999, 9999)
            ),
            format!(
                r#"{{"hrTime":[{},0],"attributes":{{"event.name":"copilot_chat.agent.turn","gen_ai.response.id":"resp-1","session.id":"conv-vscode","gen_ai.request.model":"{SONNET}","turn.index":1,{}}}}}"#,
                instant("2025-12-30T14:00:00Z").secs(),
                tokens(9999, 9999)
            ),
            format!(
                r#"{{"type":"span","traceId":"trace-1","spanId":"span-agent-1","name":"invoke_agent GitHub Copilot Chat","endTime":[{},0],"attributes":{{"gen_ai.operation.name":"invoke_agent","gen_ai.conversation.id":"conv-vscode","gen_ai.response.model":"{SONNET}",{}}}}}"#,
                instant("2025-12-30T14:00:00Z").secs(),
                tokens(9999, 9999)
            ),
            chat_span(
                ("trace-5", "span-v5"),
                instant("2025-12-30T14:05:00Z").secs(),
                &first_call,
            ),
            chat_span(
                ("trace-1", "span-v1b"),
                instant("2025-12-30T14:10:00Z").secs(),
                &format!(
                    r#"{},"gen_ai.response.id":"resp-1""#,
                    call("conv-vscode", SONNET, &tokens(1200, 150))
                ),
            ),
            format!(
                r#"{{"hrTime":[{},500000000],"spanContext":{{"traceId":"trace-2","spanId":"span-v2"}},"body":"GenAI inference: completed","attributes":{{"gen_ai.response.id":"resp-2","gen_ai.usage.input_tokens":"500","gen_ai.usage.output_tokens":"80"}}}}"#,
                instant("2025-12-31T18:00:00Z").secs()
            ),
            format!(
                r#"{{"hrTime":[{},0],"traceId":"trace-2","attributes":{{"event.name":"copilot_chat.agent.turn","copilot_chat.session_id":"conv-vscode","gen_ai.request.model":"{GPT}","copilot_chat.turn.index":2,{}}}}}"#,
                instant("2025-12-31T18:00:00Z").secs(),
                tokens(7777, 7777)
            ),
            format!(
                r#"{{"type":"span","traceId":"trace-6","spanId":"span-agent-6","name":"invoke_agent GitHub Copilot Chat","endTime":[{},0],"attributes":{{"gen_ai.conversation.id":"conv-vscode","gen_ai.response.model":"{GPT}","gen_ai.response.id":"resp-2",{}}}}}"#,
                instant("2025-12-31T18:00:00Z").secs(),
                tokens(9999, 9999)
            ),
            format!(
                r#"{{"timestamp":{},"traceId":"trace-3","_body":"copilot_chat.agent.turn completed","attributes":{{"gen_ai.response.id":"resp-3","session.id":"conv-vscode","gen_ai.request.model":"{HAIKU}","turn.index":3,{}}}}}"#,
                (instant("2026-01-01T09:00:00Z").secs()) * 1_000,
                tokens(300, 40)
            ),
            format!(
                r#"{{"type":"span","traceId":"trace-3","spanId":"span-agent-3","name":"invoke_agent GitHub Copilot Chat","endTime":[{},0],"attributes":{{"gen_ai.conversation.id":"conv-vscode","gen_ai.response.model":"{HAIKU}",{}}}}}"#,
                instant("2026-01-01T09:00:00Z").secs(),
                tokens(9999, 9999)
            ),
            format!(
                r#"{{"traceId":"trace-4","spanId":"span-agent-4","name":"invoke_agent GitHub Copilot Chat","startTime":[{},0],"attributes":{{"copilot_chat.chat_session_id":"conv-vscode","gen_ai.request.model":"{UNPRICED}",{}}}}}"#,
                instant("2026-01-01T10:00:00Z").secs(),
                tokens(600, 60)
            ),
            format!(
                r#"{{"type":"span","traceId":"trace-7","spanId":"span-v7","name":"chat {SONNET}","timeUnixNano":"{}","attributes":{{"gen_ai.conversation.id":"conv-vscode","gen_ai.response.model":"{SONNET}","gen_ai.usage.total.token_count":567,"gen_ai.usage.reasoning_tokens":5}}}}"#,
                (instant("2026-01-02T13:00:00Z").secs()) * 1_000_000_000
            ),
            chat_span(
                ("trace-8", "span-v8"),
                instant("2026-01-01T13:00:00Z").secs(),
                &call("conv-vscode", SONNET, &tokens(0, 0)),
            ),
            format!(
                r#"{{"type":"span","traceId":"trace-orphan","spanId":"span-orphan","name":"chat completion","observedTimestamp":{},"attributes":{{{}}}}}"#,
                instant("2025-12-31T12:00:00Z").secs(),
                tokens(40, 4)
            ),
        ],
    );

    let undated = write(
        "otel/undated.jsonl",
        &[format!(
            r#"{{"type":"span","traceId":"trace-undated","spanId":"span-undated","name":"chat","attributes":{{"gen_ai.operation.name":"chat"{}}}}}"#,
            call("conv-vscode", SONNET, &tokens(70, 7))
        )],
    );
    set_modified(&undated, instant("2026-01-01T06:00:00Z"));

    write("otel/empty.jsonl", &[]);
    write(
        "otel/notes.txt",
        &[chat_span(
            ("trace-txt", "span-txt"),
            instant("2025-12-31T12:00:00Z").secs(),
            &call("conv-vscode", SONNET, &tokens(9999, 9999)),
        )],
    );

    vec![
        ("COPILOT_HOME", vec![home.into_os_string()]),
        (
            "COPILOT_OTEL_FILE_EXPORTER_PATH",
            vec![exporter.into_os_string()],
        ),
    ]
}
