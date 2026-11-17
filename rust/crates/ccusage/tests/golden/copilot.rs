use std::ffi::OsString;
use std::path::Path;

use ccusage_test_support::{instant, set_modified};

const SONNET: &str = "claude-sonnet-4-20250514";
const OPUS: &str = "claude-opus-4-20250514";
const HAIKU: &str = "claude-haiku-4-5-20251001";
const GPT: &str = "gpt-5";
const UNPRICED: &str = "copilot-mystery-1";

/// One GitHub AI credit in nano-AIU.
const CREDIT: u64 = 1_000_000_000;

fn checkpoint(id: &str, timestamp: &str, credits: u64) -> String {
    format!(
        r#"{{"type":"session.usage_checkpoint","id":"{id}","timestamp":"{timestamp}","data":{{"totalNanoAiu":{},"promptCacheBreakState":[]}}}}"#,
        credits * CREDIT
    )
}

/// One `session.shutdown` line. `head` holds the members before `data` (the
/// `id` and `timestamp`, each with a trailing comma), `total` the session-wide
/// `totalNanoAiu` member with its trailing comma, and `models` the body of
/// `modelMetrics`.
fn shutdown(head: &str, total: &str, models: &str) -> String {
    format!(r#"{{"type":"session.shutdown",{head}"data":{{{total}"modelMetrics":{{{models}}}}}}}"#)
}

/// One `session.usage_record` line. `head` holds the members before `data`,
/// as for [`shutdown`], and `usage` the members of `data.usage` after
/// `interactionType`.
fn usage_record(head: &str, model: &str, usage: &str) -> String {
    format!(
        r#"{{"type":"session.usage_record",{head}"data":{{"usage":{{"model":"{model}","interactionType":"conversation-agent",{usage}}}}}}}"#
    )
}

/// One `modelMetrics` member. `usage` is the body of the usage object and
/// `rest` holds further members, each with its leading comma.
fn model(name: &str, usage: &str, rest: &str) -> String {
    format!(r#""{name}":{{"usage":{{{usage}}}{rest}}}"#)
}

/// One OpenTelemetry chat span that ended at `end_seconds`. `attributes` holds
/// the attributes after the operation name, each with its leading comma.
fn chat_span((trace_id, span_id): (&str, &str), end_seconds: i64, attributes: &str) -> String {
    format!(
        r#"{{"type":"span","traceId":"{trace_id}","spanId":"{span_id}","name":"chat","endTime":[{end_seconds},0],"attributes":{{"gen_ai.operation.name":"chat"{attributes}}}}}"#
    )
}

/// The attributes of a call of `session` to `model`, for [`chat_span`].
fn call(session: &str, model: &str, usage: &str) -> String {
    format!(r#","gen_ai.conversation.id":"{session}","gen_ai.response.model":"{model}",{usage}"#)
}

fn tokens(input: u64, output: u64) -> String {
    format!(r#""gen_ai.usage.input_tokens":{input},"gen_ai.usage.output_tokens":{output}"#)
}

pub fn seed(root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
    let home = root.join("copilot");
    let write = |relative: &str, lines: &[String]| {
        let path = home.join(relative);
        std::fs::create_dir_all(path.parent().expect("fixture file has a parent"))
            .expect("failed to create Copilot fixture directory");
        std::fs::write(&path, lines.join("\n")).expect("failed to write Copilot fixture file");
        path
    };

    // Session-state only, on a Copilot version with checkpoints. Shutdowns are
    // cumulative per (session, model), so the second one counts as the
    // difference from the first, and the `-1m` / `-1m-internal` suffixes name
    // the same model. The session total outgrows the per-model credits at every
    // snapshot, which yields cost-only `unknown` entries of 10, 5, 15 and 10
    // credits.
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
            // Replayed with the same event ID.
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
            // 2026-01-01 in Asia/Tokyo.
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
    // Not `events.jsonl`, so never read.
    write(
        "session-state/session-alpha/other.jsonl",
        &[shutdown(
            r#""id":"sd-other","timestamp":"2025-12-31T12:00:00.000Z","#,
            "",
            &model(SONNET, r#""inputTokens":9999,"outputTokens":9999"#, ""),
        )],
    );

    // Session-state and OpenTelemetry for one (session, model): the shutdown
    // intervals win, OTel rows count only after the latest shutdown visible
    // through `--until`. The first shutdown has no event ID, so its replay is
    // recognized by content. The checkpoint's 7777 credits must not surface,
    // because the session has OTel rows.
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

    // A process that died without a shutdown: OTel has three calls in the
    // second shutdown interval, which reports one request, so the OTel rows
    // replace that interval (7777 input tokens) and the first shutdown stays.
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

    // Stale day. No checkpoints, so the 20 credits the model total leaves
    // unexplained are not reported.
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

    // Checkpoints, but the shutdown's model has no credit total: the session's
    // credits cannot be reconciled and none are reported.
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

    // Never shut down: the checkpoint total is all there is.
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

    // Newer Copilot versions record every model call as a usage record, each
    // counted once at its own time. A record's input includes its cache reads
    // and writes, and the credits it carries become its cost.
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
            // 2026-01-02 in Asia/Tokyo. No recorded credits, so it is priced.
            usage_record(
                r#""id":"ur-h2","timestamp":"2026-01-01T15:30:00.000Z","#,
                GPT,
                r#""inputTokens":600,"outputTokens":60,"cacheReadTokens":200,"accounting":{"sourceSessionId":"session-eta","sequence":2,"usageId":"call-h2"},"apiCallId":"chatcmpl-h2","serviceRequestId":"svc-h2""#,
            ),
            // Without an accounting identity the event ID names the call, so
            // its replay in this file counts once.
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
    // Another session's log holding the same accounted call: it stays in the
    // session that assigned its accounting sequence.
    write(
        "session-state/session-theta/events.jsonl",
        &[accounted_record],
    );

    write("session-state/session-empty/events.jsonl", &[]);
    write(
        "session-state/session-truncated/events.jsonl",
        &[format!(
            r#"{{"type":"session.shutdown","id":"sd-t1","timestamp":"2025-12-31T12:00:00.000Z","data":{{"modelMetrics":{{"{SONNET}":{{"usage":{{"inputTokens":9999,"outp"#
        )],
    );
    // Only direct children of `session-state` are sessions.
    let undiscovered = [shutdown(
        r#""id":"sd-x1","timestamp":"2025-12-31T12:00:00.000Z","#,
        "",
        &model(SONNET, r#""inputTokens":9999,"outputTokens":9999"#, ""),
    )];
    write("session-state/nested/session-x/events.jsonl", &undiscovered);
    write("session-state/events.jsonl", &undiscovered);

    // The explicit exporter file. Its one span is also in `otel/cli/trace.jsonl`,
    // where the record IDs it carries make it the same call, counted once.
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
            // At or before the first session-beta shutdown: part of its breakdown.
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
            // Between the two session-beta shutdowns: covered by the second
            // one, unless `--until` hides that shutdown.
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
            // After the last session-beta shutdown.
            format!(
                r#"{{"type":"span","traceId":"trace-b4","spanId":"span-b4","name":"chat","time":[{},0],"attributes":{{"gen_ai.operation.name":"chat"{}}}}}"#,
                instant("2026-01-02T10:00:00Z").secs(),
                call("session-beta", GPT, &tokens(90, 9))
            ),
            // A model the session-beta shutdowns do not report.
            chat_span(
                ("trace-b1", "span-b5"),
                instant("2025-12-31T09:15:00Z").secs(),
                &call("session-beta", HAIKU, &tokens(400, 60)),
            ),
            // The calls of the session-gamma process that never shut down. The
            // first two are 2026-01-01 in Asia/Tokyo.
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
            // The OTel rows of two session-eta calls, which their usage
            // records replace: one by service request ID, one by response ID.
            // The row of another model under the same response ID is kept.
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

    // An OTel-only conversation in the VS Code shape, where one call is
    // reported by up to four record kinds: chat span, inference log, agent turn
    // log, and agent summary span, in that order of preference.
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
            // Lower-priority records of the same call, by trace or response ID.
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
            // The first call exported again under a new trace: same response ID
            // and usage, so it counts once.
            chat_span(
                ("trace-5", "span-v5"),
                instant("2025-12-30T14:05:00Z").secs(),
                &first_call,
            ),
            // Same response ID but other usage: another call of the same turn.
            chat_span(
                ("trace-1", "span-v1b"),
                instant("2025-12-30T14:10:00Z").secs(),
                &format!(
                    r#"{},"gen_ai.response.id":"resp-1""#,
                    call("conv-vscode", SONNET, &tokens(1200, 150))
                ),
            ),
            // No chat span: the inference log counts. It has neither a model
            // nor a session, which come from the agent turn log of its trace.
            // 2026-01-01 in Asia/Tokyo.
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
            // Neither a chat span nor an inference log: the agent turn log counts.
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
            // Nothing else for this trace: the agent summary span counts. It is
            // a span by its shape, not by a `type` member.
            format!(
                r#"{{"traceId":"trace-4","spanId":"span-agent-4","name":"invoke_agent GitHub Copilot Chat","startTime":[{},0],"attributes":{{"copilot_chat.chat_session_id":"conv-vscode","gen_ai.request.model":"{UNPRICED}",{}}}}}"#,
                instant("2026-01-01T10:00:00Z").secs(),
                tokens(600, 60)
            ),
            // Only a total: reported as output tokens.
            format!(
                r#"{{"type":"span","traceId":"trace-7","spanId":"span-v7","name":"chat {SONNET}","timeUnixNano":"{}","attributes":{{"gen_ai.conversation.id":"conv-vscode","gen_ai.response.model":"{SONNET}","gen_ai.usage.total.token_count":567,"gen_ai.usage.reasoning_tokens":5}}}}"#,
                (instant("2026-01-02T13:00:00Z").secs()) * 1_000_000_000
            ),
            chat_span(
                ("trace-8", "span-v8"),
                instant("2026-01-01T13:00:00Z").secs(),
                &call("conv-vscode", SONNET, &tokens(0, 0)),
            ),
            // No model and no session anywhere in the trace.
            format!(
                r#"{{"type":"span","traceId":"trace-orphan","spanId":"span-orphan","name":"chat completion","observedTimestamp":{},"attributes":{{{}}}}}"#,
                instant("2025-12-31T12:00:00Z").secs(),
                tokens(40, 4)
            ),
        ],
    );

    // No time member at all: the record is dated by the file's mtime.
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
