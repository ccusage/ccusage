use ccusage_core::ReportWindow;
use std::ffi::OsString;

use ccusage_core::SkipKind;
use ccusage_test_support::{Environment, fs_fixture};
use serde_json::json;

use super::super::parser::parse_otel_file;
use super::*;
use crate::cli::AgentReportKind;

#[test]
fn prices_the_logged_model_before_its_normalized_form() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "copilot.jsonl": json!({
            "type": "span",
            "traceId": "trace-1",
            "spanId": "span-1",
            "name": "chat",
            "endTime": [1_775_934_264_u64, 0_u64],
            "attributes": {
                "gen_ai.operation.name": "chat",
                "gen_ai.response.model": "test-model-1m-internal",
                "gen_ai.usage.input_tokens": 100,
            },
        }).to_string(),
    });
    let mut pricing = ccusage_core::PricingMap::default();
    pricing.load_json(
        r#"{
                "test-model-1m-internal":{"input_cost_per_token":1,"output_cost_per_token":1},
                "test-model":{"input_cost_per_token":2,"output_cost_per_token":2}
            }"#,
    );
    let entry = parse_otel_file(&fixture.path("copilot.jsonl"), &Diagnostics::new("copilot"))
        .unwrap()
        .pop()
        .unwrap();
    let loaded = usage_entry_to_loaded(
        entry,
        &jiff::tz::TimeZone::UTC,
        CostMode::Calculate,
        PriceTable::Calculate(&pricing),
    );
    assert_eq!(loaded.model.as_deref(), Some("test-model"));
    assert_eq!(loaded.cost, 100.0);
}

#[test]
fn only_a_single_call_selects_the_long_context_tier() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "copilot.jsonl": json!({
            "type": "span",
            "traceId": "trace-1",
            "spanId": "span-1",
            "name": "chat",
            "endTime": [1_775_934_264_u64, 0_u64],
            "attributes": {
                "gen_ai.operation.name": "chat",
                "gen_ai.response.model": "tiered-model",
                "gen_ai.usage.input_tokens": 250_000,
            },
        }).to_string(),
    });
    let mut pricing = ccusage_core::PricingMap::default();
    pricing.load_json(
        r#"{"tiered-model":{"input_cost_per_token":1,"output_cost_per_token":1,"input_cost_per_token_above_200k_tokens":2}}"#,
    );
    let call = parse_otel_file(&fixture.path("copilot.jsonl"), &Diagnostics::new("copilot"))
        .unwrap()
        .pop()
        .unwrap();
    let interval = CopilotUsageEntry {
        request_count: 3,
        ..call.clone()
    };
    let price = |entry| {
        usage_entry_to_loaded(
            entry,
            &jiff::tz::TimeZone::UTC,
            CostMode::Calculate,
            PriceTable::Calculate(&pricing),
        )
        .cost
    };

    assert_eq!(price(call), 200_000.0 + 50_000.0 * 2.0);
    assert_eq!(price(interval), 250_000.0);
}

#[test]
fn parses_copilot_chat_spans() {
    let fixture = fs_fixture!({
        "copilot.jsonl": [
            json!({ "type": "metric", "name": "gen_ai.client.token.usage" }).to_string(),
            json!({
                "type": "span",
                "traceId": "trace-1",
                "spanId": "span-1",
                "name": "chat claude-sonnet-4",
                "endTime": [1_775_934_264_u64, 967_317_833_u64],
                "attributes": {
                    "gen_ai.operation.name": "chat",
                    "gen_ai.request.model": "claude-sonnet-4",
                    "gen_ai.response.model": "claude-sonnet-4",
                    "gen_ai.conversation.id": "conv-1",
                    "gen_ai.usage.input_tokens": 19_452,
                    "gen_ai.usage.output_tokens": 281,
                    "gen_ai.usage.cache_read.input_tokens": 123,
                    "gen_ai.usage.cache_creation.input_tokens": 25,
                    "gen_ai.usage.reasoning.output_tokens": 128,
                },
            })
            .to_string(),
        ]
        .join("\n"),
    });
    let file = fixture.path("copilot.jsonl");

    let entries = parse_otel_file(&file, &Diagnostics::new("copilot")).unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(
        crate::format_rfc3339_millis(entries[0].timestamp),
        "2026-04-11T19:04:24.967Z"
    );
    assert_eq!(entries[0].session_id, "conv-1");
    assert_eq!(entries[0].model, "claude-sonnet-4");
    assert_eq!(entries[0].input_tokens, 19_304);
    assert_eq!(entries[0].output_tokens, 281);
    assert_eq!(entries[0].cache_creation_tokens, 25);
    assert_eq!(entries[0].cache_read_tokens, 123);
    assert_eq!(entries[0].reasoning_output_tokens, 128);
    assert_eq!(entries[0].dedup_key, "trace-1:span-1");
}

#[test]
fn skips_and_diagnoses_token_counts_that_exceed_the_u64_range() {
    let fixture = fs_fixture!({
        "copilot.jsonl": [
            json!({
                "type": "span",
                "traceId": "trace-huge",
                "spanId": "span-huge",
                "name": "chat claude-sonnet-4",
                "endTime": [1_775_934_264_u64, 967_317_833_u64],
                "attributes": {
                    "gen_ai.operation.name": "chat",
                    "gen_ai.response.model": "claude-sonnet-4",
                    "gen_ai.conversation.id": "conv-huge",
                    "gen_ai.usage.input_tokens": 18_446_744_073_709_551_615_u64,
                    "gen_ai.usage.output_tokens": 1,
                },
            })
            .to_string(),
            json!({
                "type": "span",
                "traceId": "trace-ok",
                "spanId": "span-ok",
                "name": "chat claude-sonnet-4",
                "endTime": [1_775_934_264_u64, 967_317_833_u64],
                "attributes": {
                    "gen_ai.operation.name": "chat",
                    "gen_ai.response.model": "claude-sonnet-4",
                    "gen_ai.conversation.id": "conv-ok",
                    "gen_ai.usage.input_tokens": 10,
                    "gen_ai.usage.output_tokens": 1,
                },
            })
            .to_string(),
        ]
        .join("\n"),
    });
    let diagnostics = Diagnostics::new("copilot");

    let entries = parse_otel_file(&fixture.path("copilot.jsonl"), &diagnostics).unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].session_id, "conv-ok");
    assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
}

#[test]
fn reports_malformed_usage_lines_and_keeps_the_next_record() {
    let fixture = fs_fixture!({
        "copilot.jsonl": [
            String::from(r#"{"attributes":{"gen_ai.usage.input_tokens": "#),
            json!({"attributes": "SECRET_CANARY"}).to_string(),
            json!({
                "type": "span",
                "traceId": "trace-healthy",
                "spanId": "span-healthy",
                "name": "chat claude-sonnet-4",
                "endTime": [1_775_934_264_u64, 967_317_833_u64],
                "attributes": {
                    "gen_ai.operation.name": "chat",
                    "gen_ai.request.model": "claude-sonnet-4",
                    "gen_ai.response.model": "claude-sonnet-4",
                    "gen_ai.conversation.id": "conv-healthy",
                    "gen_ai.usage.input_tokens": 10,
                    "gen_ai.usage.output_tokens": 5,
                },
            })
            .to_string(),
        ]
        .join("\n"),
    });
    let diagnostics = Diagnostics::new("copilot");

    let entries = parse_otel_file(&fixture.path("copilot.jsonl"), &diagnostics).unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].session_id, "conv-healthy");
    assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
    assert_eq!(diagnostics.skipped(SkipKind::UndecodedUsage), 1);
    let warnings = diagnostics.warnings(&crate::cli::SharedArgs {
        debug: true,
        ..crate::cli::SharedArgs::default()
    });
    assert!(
        warnings
            .iter()
            .all(|warning| !warning.contains("SECRET_CANARY"))
    );
}

#[test]
fn keeps_a_span_and_a_log_whose_ids_spell_the_same_string_in_two_files() {
    let fixture = fs_fixture!({
        "home/.copilot/otel/a.jsonl": json!({
            "type": "span",
            "traceId": "log:trace-1",
            "spanId": "span-1",
            "name": "chat test-model",
            "endTime": [1_767_320_400_u64, 0_u64],
            "attributes": {
                "gen_ai.operation.name": "chat",
                "gen_ai.response.model": "test-model",
                "gen_ai.conversation.id": "session-1",
                "gen_ai.usage.input_tokens": 11,
                "gen_ai.usage.output_tokens": 12
            }
        })
        .to_string(),
        "home/.copilot/otel/b.jsonl": json!({
            "hrTime": [1_767_320_460_u64, 0_u64],
            "traceId": "trace-1",
            "spanId": "span-1",
            "attributes": {
                "event.name": "gen_ai.client.inference.operation.details",
                "gen_ai.response.model": "test-model",
                "gen_ai.conversation.id": "session-2",
                "gen_ai.usage.input_tokens": 21,
                "gen_ai.usage.output_tokens": 22
            },
            "_body": "GenAI inference: test-model"
        })
        .to_string(),
    });
    Environment::with_vars([
        ("HOME", Some(OsString::from(fixture.path("home")))),
        ("USERPROFILE", None),
        ("HOMEDRIVE", None),
        ("HOMEPATH", None),
        (super::super::paths::COPILOT_HOME_ENV, None),
        (
            super::super::paths::COPILOT_OTEL_FILE_EXPORTER_PATH_ENV,
            None,
        ),
    ])
    .run(|| {
        let shared = crate::cli::SharedArgs {
            single_thread: true,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..crate::cli::SharedArgs::default()
        };

        let entries = ccusage_adapter_common::testing::entries(
            &crate::SOURCE,
            &shared,
            Default::default(),
            Some(&ccusage_core::PricingMap::default()),
        )
        .unwrap();

        let mut inputs = entries
            .iter()
            .map(|entry| entry.usage.input_tokens)
            .collect::<Vec<_>>();
        inputs.sort_unstable();
        assert_eq!(inputs, [11, 21]);
    })
}

#[test]
fn suppresses_lower_priority_records_for_same_response() {
    let fixture = fs_fixture!({
        "copilot.jsonl": [
            json!({
                "type": "span",
                "endTime": [1_775_934_263_u64, 0_u64],
                "traceId": "trace-dupe",
                "spanId": "agent-1",
                "name": "invoke_agent GitHub Copilot Chat",
                "attributes": {
                    "gen_ai.operation.name": "invoke_agent",
                    "gen_ai.response.model": "gpt-5.4-mini",
                    "gen_ai.conversation.id": "conv-dupe",
                    "gen_ai.response.id": "resp-dupe",
                    "gen_ai.usage.input_tokens": 100,
                    "gen_ai.usage.output_tokens": 30,
                },
            })
            .to_string(),
            json!({
                "hrTime": [1_775_934_263_u64, 0_u64],
                "attributes": {
                    "event.name": "gen_ai.client.inference.operation.details",
                    "gen_ai.response.model": "gpt-5.4-mini",
                    "gen_ai.response.id": "resp-dupe",
                    "gen_ai.usage.input_tokens": 80,
                    "gen_ai.usage.output_tokens": 20,
                },
                "_body": "GenAI inference: gpt-5.4-mini",
            })
            .to_string(),
            json!({
                "type": "span",
                "endTime": [1_775_934_263_u64, 0_u64],
                "traceId": "trace-dupe",
                "spanId": "chat-1",
                "name": "chat gpt-5.4-mini",
                "attributes": {
                    "gen_ai.operation.name": "chat",
                    "gen_ai.response.model": "gpt-5.4-mini",
                    "gen_ai.conversation.id": "conv-dupe",
                    "gen_ai.response.id": "resp-dupe",
                    "gen_ai.usage.input_tokens": 60,
                    "gen_ai.usage.output_tokens": 10,
                },
            })
            .to_string(),
        ]
        .join("\n"),
    });
    let file = fixture.path("copilot.jsonl");

    let entries = parse_otel_file(&file, &Diagnostics::new("copilot")).unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].dedup_key, "trace-dupe:chat-1");
    assert_eq!(entries[0].input_tokens, 60);
    assert_eq!(entries[0].output_tokens, 10);
}

fn chat_span(
    (trace_id, span_id): (&str, &str),
    end_seconds: u64,
    response_id: &str,
    input_tokens: u64,
) -> String {
    json!({
        "type": "span",
        "traceId": trace_id,
        "spanId": span_id,
        "name": "chat test-model",
        "endTime": [end_seconds, 0_u64],
        "attributes": {
            "gen_ai.operation.name": "chat",
            "gen_ai.response.model": "test-model",
            "gen_ai.conversation.id": "conv-1",
            "gen_ai.response.id": response_id,
            "gen_ai.usage.input_tokens": input_tokens,
            "gen_ai.usage.output_tokens": 10,
        },
    })
    .to_string()
}

fn dedup_keys(entries: &[CopilotUsageEntry]) -> Vec<&str> {
    entries
        .iter()
        .map(|entry| entry.dedup_key.as_str())
        .collect()
}

#[test]
fn counts_a_re_exported_chat_span_once() {
    let fixture = fs_fixture!({
        "copilot.jsonl": [
            chat_span(("trace-1", "span-1"), 1_775_934_264, "resp-1", 100),
            chat_span(("trace-1", "span-2"), 1_775_934_300, "resp-2", 200),
            chat_span(("trace-2", "span-3"), 1_775_934_368, "resp-1", 100),
            chat_span(("trace-1", "span-4"), 1_775_934_370, "resp-2", 200),
            chat_span(("trace-3", "span-5"), 1_775_934_500, "resp-3", 300),
            chat_span(("trace-1", "span-6"), 1_775_934_400, "resp-3", 300),
        ]
        .join("\n"),
    });

    let entries =
        parse_otel_file(&fixture.path("copilot.jsonl"), &Diagnostics::new("copilot")).unwrap();

    assert_eq!(
        dedup_keys(&entries),
        ["trace-1:span-1", "trace-1:span-2", "trace-1:span-6"]
    );
}

#[test]
fn bills_recorded_output_from_any_copy_of_an_otel_span() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = ccusage_core::PricingMap::default();
    pricing.load_json(r#"{"test-model":{"input_cost_per_token":1,"output_cost_per_token":2}}"#);
    for later_span in [("trace-1", "span-1"), ("trace-2", "span-2")] {
        let span = |ids, timestamp| {
            let mut record: serde_json::Value =
                serde_json::from_str(&chat_span(ids, timestamp, "resp-1", 100)).unwrap();
            record["attributes"]["gen_ai.response.model"] = json!("test-model");
            record["attributes"]["github.copilot.service_request_id"] = json!("request-1");
            record["attributes"]["gen_ai.usage.total_tokens"] = json!(110);
            record
        };
        let mut partial = span(("trace-1", "span-1"), 1_775_934_264);
        partial["attributes"]
            .as_object_mut()
            .unwrap()
            .remove("gen_ai.usage.output_tokens");
        let recorded = span(later_span, 1_775_934_300);
        let fixture = fs_fixture!({
            "copilot.jsonl": format!("{partial}\n{recorded}"),
        });
        let entries =
            parse_otel_file(&fixture.path("copilot.jsonl"), &Diagnostics::new("copilot")).unwrap();

        assert_eq!(dedup_keys(&entries), ["trace-1:span-1"]);
        let entry = usage_entry_to_loaded(
            entries.into_iter().next().unwrap(),
            &jiff::tz::TimeZone::UTC,
            CostMode::Calculate,
            PriceTable::Calculate(&pricing),
        );
        assert_eq!(entry.usage.output_tokens, 10);
        assert_eq!(entry.cost, 120.0);
    }
}

#[test]
fn keeps_calls_of_one_turn_that_share_a_response_id() {
    let fixture = fs_fixture!({
        "copilot.jsonl": [
            chat_span(("trace-1", "span-1"), 1_775_934_264, "turn-1", 1_000),
            chat_span(("trace-1", "span-2"), 1_775_934_270, "turn-1", 1_200),
            chat_span(("trace-1", "span-3"), 1_775_934_280, "turn-1", 1_400),
        ]
        .join("\n"),
    });

    let entries =
        parse_otel_file(&fixture.path("copilot.jsonl"), &Diagnostics::new("copilot")).unwrap();

    assert_eq!(
        dedup_keys(&entries),
        ["trace-1:span-1", "trace-1:span-2", "trace-1:span-3"]
    );
}

#[test]
fn does_not_double_count_reasoning_tokens() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "copilot.jsonl":
        format!(
            "{}\n",
            json!({
                "type": "span",
                "traceId": "trace-1",
                "spanId": "span-1",
                "name": "chat test-model",
                "endTime": [1_775_934_264_u64, 0_u64],
                "attributes": {
                    "gen_ai.operation.name": "chat",
                    "gen_ai.response.model": "test-model",
                    "gen_ai.conversation.id": "conv-1",
                    "gen_ai.usage.input_tokens": 100,
                    "gen_ai.usage.output_tokens": 50,
                    "gen_ai.usage.cache_read.input_tokens": 10,
                    "gen_ai.usage.cache_creation.input_tokens": 20,
                    "gen_ai.usage.reasoning.output_tokens": 5,
                },
            })
        ),
    });
    let file = fixture.path("copilot.jsonl");
    let mut pricing = ccusage_core::PricingMap::default();
    pricing.load_json(
        r#"{"test-model":{"input_cost_per_token":1,"output_cost_per_token":2,"cache_creation_input_token_cost":3,"cache_read_input_token_cost":4}}"#,
    );

    let loaded = parse_otel_file(&file, &Diagnostics::new("copilot"))
        .map(|entries| {
            entries
                .into_iter()
                .map(|entry| {
                    usage_entry_to_loaded(
                        entry,
                        &jiff::tz::TimeZone::UTC,
                        CostMode::Auto,
                        PriceTable::Auto(&pricing),
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap();
    let rows = ccusage_adapter_common::source::EntrySource::summarize(
        &crate::SOURCE,
        &loaded,
        ccusage_adapter_common::source::SummaryKind::for_report(
            AgentReportKind::Daily,
            ccusage_core::cli::WeekDay::Monday,
        ),
    )
    .unwrap();
    let report = ccusage_adapter_common::source::EntrySource::report_json(
        &crate::SOURCE,
        &rows,
        AgentReportKind::Daily,
    );

    assert_eq!(report["daily"][0]["inputTokens"], 70);
    assert_eq!(report["daily"][0]["outputTokens"], 50);
    assert_eq!(report["daily"][0]["totalTokens"], 150);
    assert_eq!(report["daily"][0]["totalCost"], 270.0);
    assert_eq!(
        report["daily"][0]["modelBreakdowns"],
        json!([{
            "modelName": "test-model",
            "inputTokens": 70,
            "outputTokens": 50,
            "cacheCreationTokens": 20,
            "cacheReadTokens": 10,
            "cost": 270.0
        }])
    );
}

#[test]
fn bills_separate_otel_reasoning_but_keeps_residual_tokens_only_in_the_total() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "copilot.jsonl": format!(
            "{}\n",
            json!({
                "type": "span",
                "traceId": "trace-1",
                "spanId": "span-1",
                "name": "chat test-model",
                "endTime": [1_775_934_264_u64, 0_u64],
                "attributes": {
                    "gen_ai.operation.name": "chat",
                    "gen_ai.response.model": "test-model",
                    "gen_ai.conversation.id": "conv-1",
                    "gen_ai.usage.input_tokens": 100,
                    "gen_ai.usage.output_tokens": 50,
                    "gen_ai.usage.cache_read.input_tokens": 10,
                    "gen_ai.usage.cache_creation.input_tokens": 20,
                    "gen_ai.usage.reasoning.output_tokens": 5,
                    "gen_ai.usage.total_tokens": 162,
                },
            })
        ),
    });
    let file = fixture.path("copilot.jsonl");
    let mut pricing = ccusage_core::PricingMap::default();
    pricing.load_json(
        r#"{"test-model":{"input_cost_per_token":1,"output_cost_per_token":2,"cache_creation_input_token_cost":3,"cache_read_input_token_cost":4}}"#,
    );

    let loaded = parse_otel_file(&file, &Diagnostics::new("copilot"))
        .map(|entries| {
            entries
                .into_iter()
                .map(|entry| {
                    usage_entry_to_loaded(
                        entry,
                        &jiff::tz::TimeZone::UTC,
                        CostMode::Auto,
                        PriceTable::Auto(&pricing),
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap();
    let rows = ccusage_adapter_common::source::EntrySource::summarize(
        &crate::SOURCE,
        &loaded,
        ccusage_adapter_common::source::SummaryKind::for_report(
            AgentReportKind::Daily,
            ccusage_core::cli::WeekDay::Monday,
        ),
    )
    .unwrap();
    let report = ccusage_adapter_common::source::EntrySource::report_json(
        &crate::SOURCE,
        &rows,
        AgentReportKind::Daily,
    );

    assert_eq!(loaded[0].extra_total_tokens, 12);
    assert_eq!(report["daily"][0]["outputTokens"], 50);
    assert_eq!(report["daily"][0]["totalTokens"], 162);
    assert_eq!(report["daily"][0]["totalCost"], 280.0);
    assert_eq!(report["daily"][0]["modelBreakdowns"][0]["cost"], 280.0);
}

#[test]
fn splits_otel_input_tokens_like_session_state() {
    let fixture = fs_fixture!({
        "copilot.jsonl": json!({
            "type": "span",
            "traceId": "trace-1",
            "spanId": "span-1",
            "name": "chat claude-haiku-4.5",
            "endTime": [1_775_934_264_u64, 0_u64],
            "attributes": {
                "gen_ai.operation.name": "chat",
                "gen_ai.response.model": "claude-haiku-4.5",
                "gen_ai.conversation.id": "session-1",
                "gen_ai.usage.input_tokens": 20_000,
                "gen_ai.usage.output_tokens": 300,
                "gen_ai.usage.cache_read.input_tokens": 12_000,
                "gen_ai.usage.cache_write.input_tokens": 7_990,
            },
        })
        .to_string(),
        "session-1/events.jsonl": json!({
            "type": "session.shutdown",
            "id": "shutdown-1",
            "timestamp": "2026-04-11T19:04:24.000Z",
            "data": {"modelMetrics": {"claude-haiku-4.5": {
                "usage": {
                    "inputTokens": 20_000,
                    "outputTokens": 300,
                    "cacheReadTokens": 12_000,
                    "cacheWriteTokens": 7_990
                },
                "requests": {"count": 1}
            }}}
        })
        .to_string(),
    });
    let split = |entry: &CopilotUsageEntry| {
        (
            entry.input_tokens,
            entry.cache_read_tokens,
            entry.cache_creation_tokens,
            entry.output_tokens,
        )
    };

    let otel =
        parse_otel_file(&fixture.path("copilot.jsonl"), &Diagnostics::new("copilot")).unwrap();
    let session_state = super::super::parser::parse_session_state_file(
        &fixture.path("session-1/events.jsonl"),
        &Diagnostics::new("copilot"),
    )
    .unwrap()
    .entries;

    assert_eq!(split(&otel[0]), (10, 12_000, 7_990, 300));
    assert_eq!(split(&otel[0]), split(&session_state[0]));
}

#[test]
fn keeps_total_only_otel_usage_out_of_output_tokens() {
    let pricing = test_model_pricing();
    let fixture = fs_fixture!({
        "copilot.jsonl":
        format!(
            "{}\n",
            json!({
                "type": "span",
                "traceId": "trace-1",
                "spanId": "span-1",
                "name": "chat test-model",
                "endTime": [1_775_934_264_u64, 0_u64],
                "attributes": {
                    "gen_ai.operation.name": "chat",
                    "gen_ai.response.model": "test-model",
                    "gen_ai.conversation.id": "conv-1",
                    "gen_ai.usage.total_tokens": 567,
                    "gen_ai.usage.reasoning_tokens": 5,
                },
            })
        ),
    });
    let file = fixture.path("copilot.jsonl");

    let mut entries = parse_otel_file(&file, &Diagnostics::new("copilot")).unwrap();

    assert_eq!(entries.len(), 1);
    let loaded = usage_entry_to_loaded(
        entries.pop().unwrap(),
        &jiff::tz::TimeZone::UTC,
        CostMode::Calculate,
        PriceTable::Calculate(&pricing),
    );
    assert_eq!(loaded.usage.output_tokens, 0);
    assert_eq!(loaded.extra_total_tokens, 567);
    assert_eq!(loaded.cost, 10.0);
}

#[test]
fn loads_session_state_tokens_and_calculates_token_cost() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": format!(
            "{}\n",
            json!({
                "type": "session.shutdown",
                "id": "shutdown-1",
                "timestamp": "2026-04-15T09:52:27.352Z",
                "data": {
                    "modelMetrics": {
                        "test-model": {
                            "usage": {
                                "inputTokens": 100,
                                "outputTokens": 50,
                                "cacheReadTokens": 10,
                                "cacheWriteTokens": 20,
                                "reasoningTokens": 5
                            },
                            "requests": {"count": 3, "cost": 999}
                        }
                    }
                }
            })
        ),
    });
    Environment::with_vars([
        ("HOME", Some(OsString::from(fixture.path("home")))),
        ("USERPROFILE", None),
        ("HOMEDRIVE", None),
        ("HOMEPATH", None),
        (super::super::paths::COPILOT_HOME_ENV, None),
        (
            super::super::paths::COPILOT_OTEL_FILE_EXPORTER_PATH_ENV,
            None,
        ),
    ]).run(|| {
    let mut pricing = ccusage_core::PricingMap::default();
    pricing.load_json(
        r#"{"test-model":{"input_cost_per_token":1,"output_cost_per_token":2,"cache_creation_input_token_cost":3,"cache_read_input_token_cost":4}}"#,
    );
    let shared = crate::cli::SharedArgs {
        mode: CostMode::Auto,
        single_thread: true,
        ..crate::cli::SharedArgs::default()
    };

    let entries = ccusage_adapter_common::testing::entries(
        &crate::SOURCE,
        &shared,
        Default::default(),
        Some(&pricing),
    )
    .unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].usage.input_tokens, 70);
    assert_eq!(entries[0].usage.output_tokens, 50);
    assert_eq!(entries[0].usage.cache_creation_input_tokens, 20);
    assert_eq!(entries[0].usage.cache_read_input_tokens, 10);
    assert_eq!(entries[0].extra_total_tokens, 0);
    assert_eq!(entries[0].message_count, Some(3));
    assert_eq!(entries[0].cost, 270.0);
    })
}

#[test]
fn uses_shutdown_snapshot_as_of_until_for_otel_reconciliation() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": [
            json!({
                "type": "session.shutdown",
                "id": "shutdown-old",
                "timestamp": "2026-01-02T01:20:00.000Z",
                "data": {"modelMetrics": {"test-model": {"usage": {
                    "inputTokens": 100,
                    "outputTokens": 50,
                    "cacheReadTokens": 10,
                    "cacheWriteTokens": 20
                }}}}
            })
            .to_string(),
            json!({
                "type": "session.shutdown",
                "id": "shutdown-latest",
                "timestamp": "2026-01-03T01:20:00.000Z",
                "data": {"modelMetrics": {"test-model": {"usage": {
                    "inputTokens": 200,
                    "outputTokens": 80,
                    "cacheReadTokens": 20,
                    "cacheWriteTokens": 30
                }}}}
            })
            .to_string(),
        ]
        .join("\n"),
        "home/.copilot/otel/otel.jsonl": json!({
            "type": "span",
            "traceId": "trace-between-shutdowns",
            "spanId": "span-between-shutdowns",
            "name": "chat test-model",
            "endTime": [1_767_320_400_u64, 0_u64],
            "attributes": {
                "gen_ai.operation.name": "chat",
                "gen_ai.response.model": "test-model",
                "gen_ai.conversation.id": "session-1",
                "gen_ai.usage.input_tokens": 11,
                "gen_ai.usage.output_tokens": 12
            }
        })
        .to_string(),
    });
    Environment::with_vars([
        ("HOME", Some(OsString::from(fixture.path("home")))),
        ("USERPROFILE", None),
        ("HOMEDRIVE", None),
        ("HOMEPATH", None),
        (super::super::paths::COPILOT_HOME_ENV, None),
        (
            super::super::paths::COPILOT_OTEL_FILE_EXPORTER_PATH_ENV,
            None,
        ),
    ])
    .run(|| {
        let shared = crate::cli::SharedArgs {
            single_thread: true,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            until: Some("20260102".parse().unwrap()),
            ..crate::cli::SharedArgs::default()
        };

        let entries = ccusage_adapter_common::testing::entries(
            &crate::SOURCE,
            &shared,
            Default::default(),
            Some(&ccusage_core::PricingMap::default()),
        )
        .unwrap();
        let mut entries = entries;
        ccusage_adapter_common::filter_loaded_entries_by_date(&mut entries, &shared);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].usage.input_tokens, 70);
        assert_eq!(entries[0].usage.output_tokens, 50);
        assert_eq!(entries[1].usage.input_tokens, 11);
        assert_eq!(entries[1].usage.output_tokens, 12);
    })
}

#[test]
fn subtracts_the_pre_since_shutdown_before_retaining_resumed_otel_rows() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": [
            json!({
                "type": "session.shutdown",
                "id": "shutdown-old",
                "timestamp": "2026-01-02T01:20:00.000Z",
                "data": {"modelMetrics": {"test-model": {
                    "usage": {
                        "inputTokens": 100,
                        "outputTokens": 50,
                        "cacheReadTokens": 10,
                        "cacheWriteTokens": 20
                    },
                    "requests": {"count": 1}
                }}}
            })
            .to_string(),
            json!({
                "type": "session.shutdown",
                "id": "shutdown-latest",
                "timestamp": "2026-01-03T01:20:00.000Z",
                "data": {"modelMetrics": {"test-model": {
                    "usage": {
                        "inputTokens": 200,
                        "outputTokens": 80,
                        "cacheReadTokens": 20,
                        "cacheWriteTokens": 30
                    },
                    "requests": {"count": 3}
                }}}
            })
            .to_string(),
        ]
        .join("\n"),
        "home/.copilot/otel/otel.jsonl": json!({
            "type": "span",
            "traceId": "trace-resumed",
            "spanId": "span-resumed",
            "name": "chat test-model",
            "endTime": [1_767_406_800_u64, 0_u64],
            "attributes": {
                "gen_ai.operation.name": "chat",
                "gen_ai.response.model": "test-model",
                "gen_ai.conversation.id": "session-1",
                "gen_ai.usage.input_tokens": 13,
                "gen_ai.usage.output_tokens": 14
            }
        })
        .to_string(),
    });
    Environment::with_vars([
        ("HOME", Some(OsString::from(fixture.path("home")))),
        ("USERPROFILE", None),
        ("HOMEDRIVE", None),
        ("HOMEPATH", None),
        (super::super::paths::COPILOT_HOME_ENV, None),
        (
            super::super::paths::COPILOT_OTEL_FILE_EXPORTER_PATH_ENV,
            None,
        ),
    ])
    .run(|| {
        let shared = crate::cli::SharedArgs {
            single_thread: true,
            since: Some("20260103".parse().unwrap()),
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..crate::cli::SharedArgs::default()
        };

        let entries = ccusage_adapter_common::testing::entries(
            &crate::SOURCE,
            &shared,
            Default::default(),
            Some(&ccusage_core::PricingMap::default()),
        )
        .unwrap();
        let mut entries = entries;
        ccusage_adapter_common::filter_loaded_entries_by_date(&mut entries, &shared);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].usage.input_tokens, 80);
        assert_eq!(entries[0].usage.output_tokens, 30);
        assert_eq!(entries[0].usage.cache_creation_input_tokens, 10);
        assert_eq!(entries[0].usage.cache_read_input_tokens, 10);
        assert_eq!(entries[0].message_count, Some(2));
        assert_eq!(entries[1].usage.input_tokens, 13);
        assert_eq!(entries[1].usage.output_tokens, 14);
        assert_eq!(entries[1].message_count, Some(1));
    })
}

#[test]
fn normalizes_copilot_model_suffixes_for_pricing_and_otel_dedupe() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": format!(
            "{}\n",
            json!({
                "type": "session.shutdown",
                "id": "shutdown-1",
                "timestamp": "2026-08-30T12:00:00Z",
                "data": {
                    "modelMetrics": {
                        "claude-opus-4.6-1m": {
                            "usage": {
                                "inputTokens": 100,
                                "outputTokens": 50,
                                "cacheReadTokens": 10,
                                "cacheWriteTokens": 20,
                                "reasoningTokens": 5
                            }
                        }
                    }
                }
            })
        ),
        "home/.copilot/otel/session.jsonl": format!(
            "{}\n",
            json!({
                "type": "span",
                "traceId": "trace-1",
                "spanId": "span-1",
                "name": "chat claude-opus-4.6-1m-internal",
                "endTime": [1_775_934_264_u64, 0_u64],
                "attributes": {
                    "gen_ai.operation.name": "chat",
                    "gen_ai.response.model": "claude-opus-4.6-1m-internal",
                    "gen_ai.conversation.id": "session-1",
                    "gen_ai.usage.input_tokens": 999,
                    "gen_ai.usage.output_tokens": 999
                }
            })
        )
    });
    Environment::with_vars([
        ("HOME", Some(OsString::from(fixture.path("home")))),
        ("USERPROFILE", None),
        ("HOMEDRIVE", None),
        ("HOMEPATH", None),
        (super::super::paths::COPILOT_HOME_ENV, None),
        (
            super::super::paths::COPILOT_OTEL_FILE_EXPORTER_PATH_ENV,
            None,
        ),
    ])
    .run(|| {
        let shared = crate::cli::SharedArgs {
            mode: CostMode::Auto,
            single_thread: true,
            ..crate::cli::SharedArgs::default()
        };

        let entries = ccusage_adapter_common::testing::entries(
            &crate::SOURCE,
            &shared,
            Default::default(),
            Some(&ccusage_core::PricingMap::load_embedded()),
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("claude-opus-4.6"));
        assert_eq!(entries[0].usage.input_tokens, 70);
        assert_eq!(entries[0].usage.output_tokens, 50);
        assert_eq!(entries[0].usage.cache_creation_input_tokens, 20);
        assert_eq!(entries[0].usage.cache_read_input_tokens, 10);
        assert_eq!(entries[0].extra_total_tokens, 0);
        assert!((entries[0].cost - 0.00173).abs() < 1e-12);
    })
}

#[test]
fn splits_cumulative_shutdowns_into_intervals_and_keeps_unmatched_otel_rows() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": [
            json!({
                "type": "session.shutdown",
                "id": "shutdown-1",
                "timestamp": "2026-04-15T09:52:27.352Z",
                "data": {"modelMetrics": {"test-model": {"usage": {
                    "inputTokens": 10,
                    "outputTokens": 20
                }}}}
            })
            .to_string(),
            json!({
                "type": "session.shutdown",
                "id": "shutdown-1",
                "timestamp": "2026-04-15T09:52:27.352Z",
                "data": {"modelMetrics": {"test-model": {"usage": {
                    "inputTokens": 10,
                    "outputTokens": 20
                }}}}
            })
            .to_string(),
            json!({
                "type": "session.shutdown",
                "id": "shutdown-2",
                "timestamp": "2026-04-15T09:53:27.352Z",
                "data": {"modelMetrics": {"test-model": {"usage": {
                    "inputTokens": 30,
                    "outputTokens": 40
                }}}}
            })
            .to_string(),
        ]
        .join("\n"),
        "home/.copilot/otel/otel.jsonl": [
            json!({
                "type": "span",
                "traceId": "trace-duplicate",
                "spanId": "span-duplicate",
                "name": "chat test-model",
                "endTime": [1_776_246_780_u64, 352_000_000_u64],
                "attributes": {
                    "gen_ai.operation.name": "chat",
                    "gen_ai.response.model": "test-model",
                    "gen_ai.conversation.id": "session-1",
                    "gen_ai.usage.input_tokens": 100,
                    "gen_ai.usage.output_tokens": 200
                }
            })
            .to_string(),
            json!({
                "type": "span",
                "traceId": "trace-post-shutdown",
                "spanId": "span-post-shutdown",
                "name": "chat test-model",
                "endTime": [1_776_246_840_u64, 0_u64],
                "attributes": {
                    "gen_ai.operation.name": "chat",
                    "gen_ai.response.model": "test-model",
                    "gen_ai.conversation.id": "session-1",
                    "gen_ai.usage.input_tokens": 7,
                    "gen_ai.usage.output_tokens": 8
                }
            })
            .to_string(),
            json!({
                "type": "span",
                "traceId": "trace-other-model",
                "spanId": "span-other-model",
                "name": "chat other-model",
                "endTime": [1_775_934_264_u64, 0_u64],
                "attributes": {
                    "gen_ai.operation.name": "chat",
                    "gen_ai.response.model": "other-model",
                    "gen_ai.conversation.id": "session-1",
                    "gen_ai.usage.input_tokens": 3,
                    "gen_ai.usage.output_tokens": 4
                }
            })
            .to_string(),
            json!({
                "type": "span",
                "traceId": "trace-other-session",
                "spanId": "span-other-session",
                "name": "chat test-model",
                "endTime": [1_775_934_264_u64, 0_u64],
                "attributes": {
                    "gen_ai.operation.name": "chat",
                    "gen_ai.response.model": "test-model",
                    "gen_ai.conversation.id": "session-2",
                    "gen_ai.usage.input_tokens": 5,
                    "gen_ai.usage.output_tokens": 6
                }
            })
            .to_string(),
        ]
        .join("\n"),
    });
    Environment::with_vars([
        ("HOME", Some(OsString::from(fixture.path("home")))),
        ("USERPROFILE", None),
        ("HOMEDRIVE", None),
        ("HOMEPATH", None),
        (super::super::paths::COPILOT_HOME_ENV, None),
        (
            super::super::paths::COPILOT_OTEL_FILE_EXPORTER_PATH_ENV,
            None,
        ),
    ])
    .run(|| {
        let shared = crate::cli::SharedArgs {
            single_thread: true,
            ..crate::cli::SharedArgs::default()
        };

        let entries = ccusage_adapter_common::testing::entries(
            &crate::SOURCE,
            &shared,
            Default::default(),
            Some(&ccusage_core::PricingMap::default()),
        )
        .unwrap();
        let rows = entries
            .iter()
            .map(|entry| {
                (
                    entry.session_id.to_string(),
                    entry.model.clone().unwrap_or_default(),
                    entry.usage.input_tokens,
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            rows,
            vec![
                ("session-1".to_string(), "other-model".to_string(), 3),
                ("session-2".to_string(), "test-model".to_string(), 5),
                ("session-1".to_string(), "test-model".to_string(), 10),
                ("session-1".to_string(), "test-model".to_string(), 20),
                ("session-1".to_string(), "test-model".to_string(), 7),
            ]
        );
    })
}

#[test]
fn splits_resumed_shutdowns_across_dates_without_double_counting() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": [
            json!({
                "type": "session.shutdown",
                "id": "shutdown-old",
                "timestamp": "2026-01-02T01:20:00.000Z",
                "data": {"modelMetrics": {"test-model": {"usage": {
                    "inputTokens": 100,
                    "outputTokens": 50,
                    "cacheReadTokens": 10,
                    "cacheWriteTokens": 20
                },
                "requests": {"count": 1}}}}
            })
            .to_string(),
            json!({
                "type": "session.shutdown",
                "id": "shutdown-latest",
                "timestamp": "2026-01-03T01:20:00.000Z",
                "data": {"modelMetrics": {"test-model": {"usage": {
                    "inputTokens": 200,
                    "outputTokens": 80,
                    "cacheReadTokens": 20,
                    "cacheWriteTokens": 30
                },
                "requests": {"count": 3}}}}
            })
            .to_string(),
        ]
        .join("\n"),
    });
    Environment::with_vars([
        ("HOME", Some(OsString::from(fixture.path("home")))),
        ("USERPROFILE", None),
        ("HOMEDRIVE", None),
        ("HOMEPATH", None),
        (super::super::paths::COPILOT_HOME_ENV, None),
        (
            super::super::paths::COPILOT_OTEL_FILE_EXPORTER_PATH_ENV,
            None,
        ),
    ])
    .run(|| {
        let shared = crate::cli::SharedArgs {
            single_thread: true,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..crate::cli::SharedArgs::default()
        };

        let entries = ccusage_adapter_common::testing::entries(
            &crate::SOURCE,
            &shared,
            Default::default(),
            Some(&ccusage_core::PricingMap::default()),
        )
        .unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].date.to_string(), "2026-01-02");
        assert_eq!(entries[0].usage.input_tokens, 70);
        assert_eq!(entries[0].usage.output_tokens, 50);
        assert_eq!(entries[0].message_count, Some(1));
        assert_eq!(entries[1].date.to_string(), "2026-01-03");
        assert_eq!(entries[1].usage.input_tokens, 80);
        assert_eq!(entries[1].usage.output_tokens, 30);
        assert_eq!(entries[1].message_count, Some(2));
        let total_input: u64 = entries.iter().map(|entry| entry.usage.input_tokens).sum();
        assert_eq!(total_input, 150);
    })
}

#[test]
fn keeps_one_entry_for_a_single_shutdown_snapshot() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": format!(
            "{}\n",
            json!({
                "type": "session.shutdown",
                "id": "shutdown-1",
                "timestamp": "2026-01-02T01:20:00.000Z",
                "data": {"modelMetrics": {"test-model": {"usage": {
                    "inputTokens": 100,
                    "outputTokens": 50
                },
                "requests": {"count": 1}}}}
            })
        ),
    });
    Environment::with_vars([
        ("HOME", Some(OsString::from(fixture.path("home")))),
        ("USERPROFILE", None),
        ("HOMEDRIVE", None),
        ("HOMEPATH", None),
        (super::super::paths::COPILOT_HOME_ENV, None),
        (
            super::super::paths::COPILOT_OTEL_FILE_EXPORTER_PATH_ENV,
            None,
        ),
    ])
    .run(|| {
        let shared = crate::cli::SharedArgs {
            single_thread: true,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..crate::cli::SharedArgs::default()
        };

        let entries = ccusage_adapter_common::testing::entries(
            &crate::SOURCE,
            &shared,
            Default::default(),
            Some(&ccusage_core::PricingMap::default()),
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date.to_string(), "2026-01-02");
        assert_eq!(entries[0].usage.input_tokens, 100);
        assert_eq!(entries[0].usage.output_tokens, 50);
        assert_eq!(entries[0].message_count, Some(1));
    })
}

const NANO_AIU_PER_CREDIT: u64 = 1_000_000_000;

fn checkpoint(id: &str, timestamp: &str, credits: u64) -> String {
    json!({
        "type": "session.usage_checkpoint",
        "id": id,
        "timestamp": timestamp,
        "data": {"totalNanoAiu": credits * NANO_AIU_PER_CREDIT}
    })
    .to_string()
}

fn shutdown(id: &str, timestamp: &str, credits: u64, model_credits: Option<u64>) -> String {
    let mut model = json!({"usage": {"inputTokens": 100, "outputTokens": 50}});
    if let Some(model_credits) = model_credits {
        model["totalNanoAiu"] = json!(model_credits * NANO_AIU_PER_CREDIT);
    }
    json!({
        "type": "session.shutdown",
        "id": id,
        "timestamp": timestamp,
        "data": {
            "totalNanoAiu": credits * NANO_AIU_PER_CREDIT,
            "modelMetrics": {"test-model": model}
        }
    })
    .to_string()
}

fn load_copilot_home(
    fixture: &ccusage_test_support::Fixture,
    shared: &crate::cli::SharedArgs,
) -> Vec<LoadedEntry> {
    load_priced_copilot_home(fixture, shared, &ccusage_core::PricingMap::default())
}

fn load_priced_copilot_home(
    fixture: &ccusage_test_support::Fixture,
    shared: &crate::cli::SharedArgs,
    pricing: &ccusage_core::PricingMap,
) -> Vec<LoadedEntry> {
    Environment::with_vars([
        ("HOME", Some(OsString::from(fixture.path("home")))),
        ("USERPROFILE", None),
        ("HOMEDRIVE", None),
        ("HOMEPATH", None),
        (super::super::paths::COPILOT_HOME_ENV, None),
        (
            super::super::paths::COPILOT_OTEL_FILE_EXPORTER_PATH_ENV,
            None,
        ),
    ])
    .run(|| {
        let mut entries = ccusage_adapter_common::testing::entries(
            &crate::SOURCE,
            shared,
            Default::default(),
            Some(pricing),
        )
        .unwrap();
        ccusage_adapter_common::filter_loaded_entries_by_date(&mut entries, shared);
        entries
    })
}

fn utc_args() -> crate::cli::SharedArgs {
    crate::cli::SharedArgs {
        single_thread: true,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..crate::cli::SharedArgs::default()
    }
}

fn credit_rows(entries: &[LoadedEntry]) -> Vec<(String, String, u64, u64)> {
    entries
        .iter()
        .map(|entry| {
            (
                entry.date.to_string(),
                entry.model.clone().unwrap_or_default(),
                ccusage_core::total_usage_tokens(entry.usage),
                (entry.cost * 100.0).round() as u64,
            )
        })
        .collect()
}

#[test]
fn reports_session_credits_missing_from_model_metrics() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": [
            checkpoint("checkpoint-1", "2026-01-02T10:00:00.000Z", 51),
            checkpoint("checkpoint-2", "2026-01-02T10:10:00.000Z", 73),
            shutdown("shutdown-1", "2026-01-02T10:20:00.000Z", 93, Some(42)),
        ]
        .join("\n"),
    });

    let entries = load_copilot_home(&fixture, &utc_args());

    assert_eq!(
        credit_rows(&entries),
        [
            ("2026-01-02".to_string(), "test-model".to_string(), 150, 42),
            ("2026-01-02".to_string(), "unknown".to_string(), 0, 51),
        ]
    );
    assert_eq!(entries[1].message_count, None);
    assert_eq!(entries[1].missing_pricing_model, None);
}

#[test]
fn reports_checkpoint_credits_of_sessions_without_a_shutdown() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": [
            shutdown("shutdown-1", "2026-01-02T10:00:00.000Z", 10, Some(10)),
            checkpoint("checkpoint-1", "2026-01-02T11:00:00.000Z", 25),
            checkpoint("checkpoint-2", "2026-01-03T11:00:00.000Z", 40),
        ]
        .join("\n"),
        "home/.copilot/session-state/session-2/events.jsonl":
            checkpoint("checkpoint-3", "2026-01-03T12:00:00.000Z", 7),
    });

    let entries = load_copilot_home(&fixture, &utc_args());

    assert_eq!(
        credit_rows(&entries),
        [
            ("2026-01-02".to_string(), "test-model".to_string(), 150, 10),
            ("2026-01-02".to_string(), "unknown".to_string(), 0, 15),
            ("2026-01-03".to_string(), "unknown".to_string(), 0, 15),
            ("2026-01-03".to_string(), "unknown".to_string(), 0, 7),
        ]
    );
}

#[test]
fn reports_resumed_session_credits_when_the_last_shutdown_has_no_model_metrics() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": [
            checkpoint("checkpoint-1", "2026-01-02T10:00:00.000Z", 1_072),
            json!({
                "type": "session.resume",
                "timestamp": "2026-01-03T09:00:00.000Z",
                "data": {}
            })
            .to_string(),
            json!({
                "type": "session.shutdown",
                "id": "shutdown-1",
                "timestamp": "2026-01-03T10:00:00.000Z",
                "data": {"totalNanoAiu": 1_072 * NANO_AIU_PER_CREDIT, "modelMetrics": {}}
            })
            .to_string(),
        ]
        .join("\n"),
    });

    let entries = load_copilot_home(&fixture, &utc_args());

    assert_eq!(
        credit_rows(&entries),
        [("2026-01-03".to_string(), "unknown".to_string(), 0, 1_072)]
    );
}

#[test]
fn subtracts_credit_snapshots_before_since() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": [
            shutdown("shutdown-1", "2026-01-02T10:00:00.000Z", 30, Some(10)),
            checkpoint("checkpoint-1", "2026-01-03T10:00:00.000Z", 50),
        ]
        .join("\n"),
    });
    let shared = crate::cli::SharedArgs {
        since: Some("20260103".parse().unwrap()),
        ..utc_args()
    };

    let entries = load_copilot_home(&fixture, &shared);

    assert_eq!(
        credit_rows(&entries),
        [("2026-01-03".to_string(), "unknown".to_string(), 0, 20)]
    );
}

#[test]
fn does_not_count_credits_that_a_later_shutdown_attributes() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": [
            checkpoint("checkpoint-1", "2026-01-02T09:00:00.000Z", 30),
            shutdown("shutdown-1", "2026-01-02T10:00:00.000Z", 30, Some(30)),
            checkpoint("checkpoint-2", "2026-01-02T11:00:00.000Z", 50),
            shutdown("shutdown-2", "2026-01-02T12:00:00.000Z", 50, Some(35)),
            shutdown("shutdown-3", "2026-01-02T13:00:00.000Z", 50, Some(50)),
        ]
        .join("\n"),
    });

    let entries = load_copilot_home(&fixture, &utc_args());

    assert_eq!(
        credit_rows(&entries),
        [
            ("2026-01-02".to_string(), "test-model".to_string(), 150, 30),
            ("2026-01-02".to_string(), "test-model".to_string(), 0, 5),
            ("2026-01-02".to_string(), "test-model".to_string(), 0, 15),
        ]
    );
}

#[test]
fn credit_gaps_do_not_depend_on_until() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": [
            checkpoint("checkpoint-1", "2026-01-02T09:00:00.000Z", 30),
            shutdown("shutdown-1", "2026-01-02T10:00:00.000Z", 30, Some(30)),
            checkpoint("checkpoint-2", "2026-01-02T11:00:00.000Z", 50),
            shutdown("shutdown-2", "2026-01-02T12:00:00.000Z", 50, Some(35)),
            shutdown("shutdown-3", "2026-01-03T10:00:00.000Z", 50, Some(50)),
        ]
        .join("\n"),
    });
    let shared = crate::cli::SharedArgs {
        until: Some("20260102".parse().unwrap()),
        ..utc_args()
    };

    let entries = load_copilot_home(&fixture, &shared);

    assert_eq!(
        credit_rows(&entries),
        [
            ("2026-01-02".to_string(), "test-model".to_string(), 150, 30),
            ("2026-01-02".to_string(), "test-model".to_string(), 0, 5),
        ]
    );
}

#[test]
fn leaves_sessions_without_checkpoints_unchanged() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": [
            shutdown("shutdown-1", "2026-01-02T10:00:00.000Z", 60, Some(40)),
            shutdown("shutdown-2", "2026-01-03T10:00:00.000Z", 25, Some(25)),
        ]
        .join("\n"),
    });

    let entries = load_copilot_home(&fixture, &utc_args());

    assert_eq!(
        credit_rows(&entries),
        [("2026-01-02".to_string(), "test-model".to_string(), 150, 40)]
    );
}

#[test]
fn leaves_sessions_with_incomplete_credit_totals_unchanged() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": [
            checkpoint("checkpoint-1", "2026-01-02T09:00:00.000Z", 20),
            shutdown("shutdown-1", "2026-01-02T10:00:00.000Z", 93, None),
            checkpoint("checkpoint-2", "2026-01-02T11:00:00.000Z", 120),
        ]
        .join("\n"),
    });

    let entries = load_copilot_home(&fixture, &utc_args());

    assert_eq!(
        credit_rows(&entries),
        [("2026-01-02".to_string(), "test-model".to_string(), 150, 0)]
    );
}

#[test]
fn reconciles_partial_and_complete_exports_against_shutdown_credits() {
    for (complete, total, expected_gap) in [(false, 30, 20), (true, 10, 0)] {
        let mut summary: serde_json::Value = serde_json::from_str(&shutdown_with_requests(
            "shutdown-1",
            "2026-01-03T10:00:00Z",
            2,
        ))
        .unwrap();
        summary["data"]["totalNanoAiu"] = json!(total * NANO_AIU_PER_CREDIT);
        summary["data"]["modelMetrics"]["test-model"]["totalNanoAiu"] =
            json!(10 * NANO_AIU_PER_CREDIT);
        let mut spans = vec![
            otel_call("span-1", "2026-01-03T09:00:00Z"),
            otel_call("resumed-span", "2026-01-04T09:00:00Z"),
        ];
        if complete {
            spans.push(otel_call("span-2", "2026-01-03T09:30:00Z"));
        }
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-1/events.jsonl": [
                checkpoint("initial", "2026-01-02T10:00:00Z", total),
                summary.to_string(),
                checkpoint("resumed", "2026-01-04T10:00:00Z", total + 5),
            ].join("\n"),
            "home/.copilot/otel/otel.jsonl": spans.join("\n"),
        });

        let all = load_copilot_home(&fixture, &utc_args());
        let bounded = load_copilot_home(
            &fixture,
            &crate::cli::SharedArgs {
                since: Some("20260103".parse().unwrap()),
                until: Some("20260103".parse().unwrap()),
                ..utc_args()
            },
        );

        let mut expected = vec![("2026-01-03".to_string(), "test-model".to_string(), 220, 10)];
        if expected_gap > 0 {
            expected.push((
                "2026-01-03".to_string(),
                "unknown".to_string(),
                0,
                expected_gap,
            ));
        }
        assert_eq!(
            credit_rows(&bounded),
            expected,
            "complete export: {complete}"
        );
        expected.push(("2026-01-04".to_string(), "test-model".to_string(), 2, 0));
        assert_eq!(credit_rows(&all), expected, "complete export: {complete}");
    }
}

#[test]
fn reconciles_watermarked_receipts_once_before_reporting_checkpoint_residuals() {
    let receipt = |id: &str, timestamp: &str, sequence: u64, credits: Option<u64>| {
        usage_record(
            id,
            timestamp,
            json!({
                "apiCallId": id,
                "accounting": {"sourceSessionId": "session-1", "sequence": sequence},
                "copilotUsage": {"totalNanoAiu": credits.map(|value| value * NANO_AIU_PER_CREDIT)},
            }),
        )
    };
    let checkpoint_with_watermark = |id, timestamp, total, sequence| {
        let mut value: serde_json::Value =
            serde_json::from_str(&checkpoint(id, timestamp, total)).unwrap();
        value["data"]["usageAccountingWatermarks"] = json!({"session-1": sequence});
        value.to_string()
    };
    for (second_credit, watermark, expected_gap) in
        [(Some(5), 2, 20), (None, 2, 0), (Some(5), 3, 0)]
    {
        let first = receipt("response-a", "2026-01-02T09:00:00Z", 1, Some(10));
        let second = receipt("response-b", "2026-01-03T09:00:00Z", 2, second_credit);
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-1/events.jsonl": [
                shutdown("legacy", "2026-01-01T09:00:00Z", 4, Some(4)),
                first.clone(),
                checkpoint_with_watermark("first", "2026-01-02T10:00:00Z", 14, 1),
                second,
                checkpoint_with_watermark("second", "2026-01-03T10:00:00Z", 39, watermark),
            ].join("\n"),
            "home/.copilot/session-state/copy/events.jsonl": first,
            "home/.copilot/otel/otel.jsonl": otel_call_with(
                "span-1", "2026-01-02T09:00:00Z", ("gen_ai.response.id", "response-a"),
            ),
        });
        let all = load_copilot_home(&fixture, &utc_args());
        let bounded = load_copilot_home(
            &fixture,
            &crate::cli::SharedArgs {
                since: Some("20260103".parse().unwrap()),
                ..utc_args()
            },
        );

        let mut expected = vec![(
            "2026-01-03".to_string(),
            "test-model".to_string(),
            100,
            second_credit.unwrap_or_default(),
        )];
        if expected_gap > 0 {
            expected.push((
                "2026-01-03".to_string(),
                "unknown".to_string(),
                0,
                expected_gap,
            ));
        }
        assert_eq!(
            credit_rows(&bounded),
            expected,
            "watermark {watermark}, credits {second_credit:?}"
        );
        expected.insert(
            0,
            ("2026-01-02".to_string(), "test-model".to_string(), 100, 10),
        );
        expected.insert(
            0,
            ("2026-01-01".to_string(), "test-model".to_string(), 150, 4),
        );
        assert_eq!(
            credit_rows(&all),
            expected,
            "watermark {watermark}, credits {second_credit:?}"
        );
    }
}

#[test]
fn leaves_receipted_sessions_unchanged_when_a_watermarked_shutdown_lacks_its_total() {
    let receipt = usage_record(
        "response-a",
        "2026-01-02T09:00:00Z",
        json!({
            "apiCallId": "response-a",
            "accounting": {"sourceSessionId": "session-1", "sequence": 1},
            "copilotUsage": {"totalNanoAiu": 10 * NANO_AIU_PER_CREDIT},
        }),
    );
    let mut watermarked_checkpoint: serde_json::Value =
        serde_json::from_str(&checkpoint("checkpoint-1", "2026-01-02T10:00:00Z", 30)).unwrap();
    watermarked_checkpoint["data"]["usageAccountingWatermarks"] = json!({"session-1": 1});
    let untotalled_shutdown = json!({
        "type": "session.shutdown",
        "id": "shutdown-1",
        "timestamp": "2026-01-02T11:00:00Z",
        "data": {"usageAccountingWatermarks": {"session-1": 1}}
    });
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": [
            receipt,
            watermarked_checkpoint.to_string(),
            untotalled_shutdown.to_string(),
        ].join("\n"),
    });

    let entries = load_copilot_home(&fixture, &utc_args());

    assert_eq!(
        credit_rows(&entries),
        [("2026-01-02".to_string(), "test-model".to_string(), 100, 10)]
    );
}

#[test]
fn leaves_receipted_sessions_unchanged_when_a_later_shutdown_has_no_watermark() {
    let totaled = shutdown("shutdown-1", "2026-01-02T11:00:00Z", 30, Some(30));
    let mut untotalled: serde_json::Value = serde_json::from_str(&totaled).unwrap();
    untotalled["data"]
        .as_object_mut()
        .unwrap()
        .remove("totalNanoAiu");
    for later_shutdown in [totaled, untotalled.to_string()] {
        let receipt = usage_record(
            "response-a",
            "2026-01-02T09:00:00Z",
            json!({
                "apiCallId": "response-a",
                "accounting": {"sourceSessionId": "session-1", "sequence": 1},
                "copilotUsage": {"totalNanoAiu": 10 * NANO_AIU_PER_CREDIT},
            }),
        );
        let mut watermarked_checkpoint: serde_json::Value =
            serde_json::from_str(&checkpoint("checkpoint-1", "2026-01-02T10:00:00Z", 30)).unwrap();
        watermarked_checkpoint["data"]["usageAccountingWatermarks"] = json!({"session-1": 1});
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-1/events.jsonl": [
                receipt,
                watermarked_checkpoint.to_string(),
                later_shutdown.clone(),
            ].join("\n"),
        });

        let entries = load_copilot_home(&fixture, &utc_args());

        assert_eq!(
            credit_rows(&entries),
            [("2026-01-02".to_string(), "test-model".to_string(), 100, 10)],
            "{later_shutdown}"
        );
    }
}

#[test]
fn leaves_the_gap_unknown_when_otel_has_no_recorded_credits() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl":
            checkpoint("checkpoint-1", "2026-01-02T10:00:00.000Z", 20),
        "home/.copilot/otel/otel.jsonl": json!({
            "type": "span",
            "traceId": "trace-1",
            "spanId": "span-1",
            "name": "chat test-model",
            "endTime": [1_767_348_000_u64, 0_u64],
            "attributes": {
                "gen_ai.operation.name": "chat",
                "gen_ai.response.model": "test-model",
                "gen_ai.conversation.id": "session-1",
                "gen_ai.usage.input_tokens": 11,
                "gen_ai.usage.output_tokens": 12
            }
        })
        .to_string(),
    });

    let entries = load_priced_copilot_home(&fixture, &utc_args(), &test_model_pricing());

    assert_eq!(
        credit_rows(&entries),
        [(
            "2026-01-02".to_string(),
            "test-model".to_string(),
            23,
            3_500
        )]
    );
}

#[test]
fn calculate_mode_does_not_price_credit_only_entries() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl":
            checkpoint("checkpoint-1", "2026-01-02T10:00:00.000Z", 20),
    });
    let shared = crate::cli::SharedArgs {
        mode: CostMode::Calculate,
        ..utc_args()
    };

    let entries = load_copilot_home(&fixture, &shared);

    assert_eq!(
        credit_rows(&entries),
        [("2026-01-02".to_string(), "unknown".to_string(), 0, 0)]
    );
}

fn shutdown_with_requests(id: &str, timestamp: &str, requests: u64) -> String {
    json!({
        "type": "session.shutdown",
        "id": id,
        "timestamp": timestamp,
        "data": {"modelMetrics": {"test-model": {
            "usage": {"inputTokens": requests * 100, "outputTokens": requests * 10},
            "requests": {"count": requests}
        }}}
    })
    .to_string()
}

fn otel_call(span_id: &str, timestamp: &str) -> String {
    let seconds = crate::parse_ts_timestamp(timestamp).unwrap().as_millis() / 1_000;
    json!({
        "type": "span",
        "traceId": "trace-1",
        "spanId": span_id,
        "name": "chat test-model",
        "endTime": [seconds, 0_u64],
        "attributes": {
            "gen_ai.operation.name": "chat",
            "gen_ai.response.model": "test-model",
            "gen_ai.conversation.id": "session-1",
            "gen_ai.usage.input_tokens": 1,
            "gen_ai.usage.output_tokens": 1
        }
    })
    .to_string()
}

fn usage_rows(entries: &[LoadedEntry]) -> Vec<(String, u64)> {
    entries
        .iter()
        .map(|entry| {
            (
                crate::format_rfc3339_millis(entry.timestamp),
                entry.usage.input_tokens,
            )
        })
        .collect()
}

fn rows_on(rows: &[(String, u64)], date: &str) -> Vec<(String, u64)> {
    rows.iter()
        .filter(|(timestamp, _)| timestamp.starts_with(date))
        .cloned()
        .collect()
}

fn crashed_process_fixture() -> ccusage_test_support::Fixture {
    fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": [
            shutdown_with_requests("shutdown-1", "2026-01-02T10:00:00.000Z", 2),
            shutdown_with_requests("shutdown-2", "2026-01-03T10:00:00.000Z", 3),
        ]
        .join("\n"),
        "home/.copilot/otel/otel.jsonl": [
            otel_call("first-1", "2026-01-02T09:00:00.000Z"),
            otel_call("first-2", "2026-01-02T09:30:00.000Z"),
            otel_call("crashed-1", "2026-01-02T20:00:00.000Z"),
            otel_call("crashed-2", "2026-01-02T21:00:00.000Z"),
            otel_call("crashed-3", "2026-01-03T08:00:00.000Z"),
            otel_call("restored-1", "2026-01-03T09:00:00.000Z"),
            otel_call("after-shutdown", "2026-01-03T11:00:00.000Z"),
        ]
        .join("\n"),
    })
}

#[test]
fn reports_otel_calls_of_a_process_that_ended_without_a_shutdown() {
    let fixture = crashed_process_fixture();

    let entries = load_copilot_home(&fixture, &utc_args());

    assert_eq!(
        usage_rows(&entries),
        [
            ("2026-01-02T09:00:00.000Z".to_string(), 1),
            ("2026-01-02T09:30:00.000Z".to_string(), 1),
            ("2026-01-02T20:00:00.000Z".to_string(), 1),
            ("2026-01-02T21:00:00.000Z".to_string(), 1),
            ("2026-01-03T08:00:00.000Z".to_string(), 1),
            ("2026-01-03T09:00:00.000Z".to_string(), 1),
            ("2026-01-03T11:00:00.000Z".to_string(), 1),
        ]
    );
}

#[test]
fn otel_reconciliation_does_not_depend_on_the_date_range() {
    let fixture = crashed_process_fixture();
    let full = usage_rows(&load_copilot_home(&fixture, &utc_args()));
    let until = crate::cli::SharedArgs {
        until: Some("20260102".parse().unwrap()),
        ..utc_args()
    };
    let since = crate::cli::SharedArgs {
        since: Some("20260103".parse().unwrap()),
        ..utc_args()
    };

    assert_eq!(
        usage_rows(&load_copilot_home(&fixture, &until)),
        rows_on(&full, "2026-01-02")
    );
    assert_eq!(
        usage_rows(&load_copilot_home(&fixture, &since)),
        rows_on(&full, "2026-01-03")
    );
}

#[test]
fn keeps_shutdown_usage_from_before_the_otel_export_started() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": [
            shutdown_with_requests("shutdown-1", "2026-01-02T10:00:00.000Z", 2),
            shutdown_with_requests("shutdown-2", "2026-01-03T10:00:00.000Z", 3),
        ]
        .join("\n"),
        "home/.copilot/otel/otel.jsonl": [
            otel_call("crashed-1", "2026-01-02T20:00:00.000Z"),
            otel_call("crashed-2", "2026-01-02T21:00:00.000Z"),
            otel_call("restored-1", "2026-01-03T09:00:00.000Z"),
        ]
        .join("\n"),
    });

    let entries = load_copilot_home(&fixture, &utc_args());

    assert_eq!(
        usage_rows(&entries),
        [
            ("2026-01-02T10:00:00.000Z".to_string(), 200),
            ("2026-01-02T20:00:00.000Z".to_string(), 1),
            ("2026-01-02T21:00:00.000Z".to_string(), 1),
            ("2026-01-03T09:00:00.000Z".to_string(), 1),
        ]
    );
}

#[test]
fn keeps_shutdown_intervals_when_otel_has_no_calls_they_miss() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": [
            shutdown_with_requests("shutdown-1", "2026-01-02T10:00:00.000Z", 2),
            shutdown_with_requests("shutdown-2", "2026-01-03T10:00:00.000Z", 3),
        ]
        .join("\n"),
        "home/.copilot/otel/otel.jsonl": [
            otel_call("first-1", "2026-01-02T09:00:00.000Z"),
            otel_call("first-2", "2026-01-02T09:30:00.000Z"),
            otel_call("second-1", "2026-01-03T09:00:00.000Z"),
            otel_call("after-shutdown", "2026-01-03T11:00:00.000Z"),
        ]
        .join("\n"),
    });

    let entries = load_copilot_home(&fixture, &utc_args());

    assert_eq!(
        usage_rows(&entries),
        [
            ("2026-01-02T10:00:00.000Z".to_string(), 200),
            ("2026-01-03T10:00:00.000Z".to_string(), 100),
            ("2026-01-03T11:00:00.000Z".to_string(), 1),
        ]
    );
}

#[test]
fn does_not_double_count_calls_a_concurrent_process_shuts_down_with() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": [
            shutdown_with_requests("shutdown-1", "2026-01-02T10:00:00.000Z", 1),
            shutdown_with_requests("shutdown-b", "2026-01-02T10:55:00.000Z", 1),
            shutdown_with_requests("shutdown-a", "2026-01-02T11:00:00.000Z", 3),
        ]
        .join("\n"),
        "home/.copilot/otel/otel.jsonl": [
            otel_call("first-1", "2026-01-02T09:00:00.000Z"),
            otel_call("a-1", "2026-01-02T10:40:00.000Z"),
            otel_call("a-2", "2026-01-02T10:50:00.000Z"),
        ]
        .join("\n"),
    });

    let entries = load_copilot_home(&fixture, &utc_args());

    assert_eq!(
        usage_rows(&entries),
        [
            ("2026-01-02T10:00:00.000Z".to_string(), 100),
            ("2026-01-02T11:00:00.000Z".to_string(), 200),
        ]
    );
}

#[test]
fn leaves_shutdowns_without_request_counts_to_the_shutdown_breakdown() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": json!({
            "type": "session.shutdown",
            "id": "shutdown-1",
            "timestamp": "2026-01-02T10:00:00.000Z",
            "data": {"modelMetrics": {"test-model": {"usage": {"inputTokens": 100}}}}
        })
        .to_string(),
        "home/.copilot/otel/otel.jsonl": [
            otel_call("first-1", "2026-01-02T09:00:00.000Z"),
            otel_call("first-2", "2026-01-02T09:30:00.000Z"),
        ]
        .join("\n"),
    });

    let entries = load_copilot_home(&fixture, &utc_args());

    assert_eq!(
        usage_rows(&entries),
        [("2026-01-02T10:00:00.000Z".to_string(), 100)]
    );
}

const RECEIPTS_ONLY: &str =
    include_str!("../../tests/fixtures/session-state/session-receipts/events.jsonl");
const SHUTDOWN_ONLY: &str =
    include_str!("../../tests/fixtures/session-state/session-a/events.jsonl");
const SHUTDOWN_THEN_RECEIPTS: &str =
    include_str!("../../tests/fixtures/session-state/session-resumed-receipts/events.jsonl");

fn token_rows(entries: &[LoadedEntry]) -> Vec<(String, String, u64, u64, u64, u64, u64)> {
    entries
        .iter()
        .map(|entry| {
            let usage = entry.usage;
            (
                crate::format_rfc3339_millis(entry.timestamp),
                entry.model.clone().unwrap_or_default(),
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_creation_input_tokens,
                usage.cache_read_input_tokens,
                entry.message_count.unwrap_or_default(),
            )
        })
        .collect()
}

fn row(
    timestamp: &str,
    model: &str,
    tokens: [u64; 5],
) -> (String, String, u64, u64, u64, u64, u64) {
    let [input, output, cache_write, cache_read, requests] = tokens;
    (
        timestamp.to_string(),
        model.to_string(),
        input,
        output,
        cache_write,
        cache_read,
        requests,
    )
}

#[test]
fn reports_each_usage_record_as_one_call_at_its_own_time() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-receipts/events.jsonl": RECEIPTS_ONLY,
    });

    let entries = load_copilot_home(&fixture, &utc_args());

    assert_eq!(
        token_rows(&entries),
        [
            row(
                "2026-02-05T09:00:00.000Z",
                "claude-sonnet-4.6",
                [100, 80, 100, 1000, 1]
            ),
            row("2026-02-05T09:05:00.000Z", "gpt-5.4", [100, 40, 0, 200, 1]),
            row(
                "2026-02-06T10:00:00.000Z",
                "claude-sonnet-4.6",
                [50, 60, 50, 400, 1]
            ),
        ]
    );
}

#[test]
fn keeps_shutdown_intervals_for_sessions_without_usage_records() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-a/events.jsonl": SHUTDOWN_ONLY,
    });

    let entries = load_copilot_home(&fixture, &utc_args());

    assert_eq!(
        token_rows(&entries),
        [
            row(
                "2026-01-02T01:20:00.000Z",
                "claude-opus-4.6",
                [70, 50, 20, 10, 1]
            ),
            row(
                "2026-01-03T01:20:00.000Z",
                "claude-opus-4.6",
                [80, 30, 10, 10, 2]
            ),
        ]
    );
}

#[test]
fn counts_calls_once_when_a_shutdown_follows_their_usage_records() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-resumed-receipts/events.jsonl":
            SHUTDOWN_THEN_RECEIPTS,
    });

    let entries = load_copilot_home(&fixture, &utc_args());

    assert_eq!(
        token_rows(&entries),
        [
            row(
                "2026-02-07T09:00:00.000Z",
                "claude-sonnet-4.6",
                [100, 40, 100, 300, 2]
            ),
            row(
                "2026-02-08T23:00:00.000Z",
                "claude-sonnet-4.6",
                [50, 30, 0, 200, 1]
            ),
        ]
    );
}

#[test]
fn counts_a_usage_record_without_tokens_as_a_call() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": [
            json!({
                "type": "session.usage_record",
                "id": "record-1",
                "timestamp": "2026-02-05T09:00:00.000Z",
                "data": {"usage": {
                    "model": "gpt-5.4",
                    "inputTokens": 0,
                    "outputTokens": 0,
                    "accounting": {"sourceSessionId": "session-1", "sequence": 1}
                }}
            })
            .to_string(),
            checkpoint("checkpoint-1", "2026-02-05T09:00:01.000Z", 3),
            json!({
                "type": "session.shutdown",
                "id": "shutdown-1",
                "timestamp": "2026-02-06T09:00:00.000Z",
                "data": {
                    "totalNanoAiu": 3 * NANO_AIU_PER_CREDIT,
                    "modelMetrics": {"gpt-5.4": {
                        "usage": {"inputTokens": 0, "outputTokens": 0},
                        "requests": {"count": 1}
                    }}
                }
            })
            .to_string(),
        ]
        .join("\n"),
    });

    let entries = load_copilot_home(&fixture, &utc_args());

    assert_eq!(
        token_rows(&entries),
        [row("2026-02-05T09:00:00.000Z", "gpt-5.4", [0, 0, 0, 0, 1])]
    );
}

#[test]
fn counts_a_usage_record_delivered_twice_once() {
    let duplicated = RECEIPTS_ONLY
        .lines()
        .find(|line| line.contains("\"record-r-2\""))
        .unwrap()
        .replace("\"record-r-2\"", "\"record-r-2-again\"");
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-receipts/events.jsonl":
            format!("{RECEIPTS_ONLY}{duplicated}\n"),
    });

    let entries = load_copilot_home(&fixture, &utc_args());

    assert_eq!(token_rows(&entries).len(), 3);
}

fn usage_record(id: &str, timestamp: &str, usage: serde_json::Value) -> String {
    let mut usage_json = json!({"model": "test-model", "inputTokens": 100});
    usage_json
        .as_object_mut()
        .unwrap()
        .extend(usage.as_object().unwrap().clone());
    json!({
        "type": "session.usage_record",
        "id": id,
        "timestamp": timestamp,
        "data": {"usage": usage_json}
    })
    .to_string()
}

#[test]
fn counts_a_usage_record_in_parent_and_child_files_once_for_its_source_session() {
    for (parent, child) in [("a-parent", "b-child"), ("b-parent", "a-child")] {
        let record = |id: &str| {
            usage_record(
                id,
                "2026-01-02T10:00:00.000Z",
                json!({"accounting": {"sourceSessionId": child, "sequence": 7}}),
            )
        };
        let fixture = fs_fixture!({});
        for (session, event_id) in [(parent, "parent-event"), (child, "child-event")] {
            fixture.write_file(
                format!("home/.copilot/session-state/{session}/events.jsonl"),
                record(event_id),
            );
        }

        let entries = load_copilot_home(&fixture, &utc_args());

        assert_eq!(
            entries
                .iter()
                .map(|entry| (entry.session_id.to_string(), entry.usage.input_tokens))
                .collect::<Vec<_>>(),
            [(child.to_string(), 100)],
            "parent {parent}, child {child}"
        );
    }
}

fn parent_snapshots(watermarks: Option<serde_json::Value>) -> Vec<String> {
    let mut checkpoint = json!({
        "type": "session.usage_checkpoint",
        "id": "checkpoint-1",
        "timestamp": "2026-01-02T10:30:00.000Z",
        "data": {"totalNanoAiu": 2 * NANO_AIU_PER_CREDIT}
    });
    let mut shutdown: serde_json::Value = serde_json::from_str(&shutdown_with_requests(
        "shutdown-1",
        "2026-01-02T11:00:00.000Z",
        1,
    ))
    .unwrap();
    shutdown["data"]["totalNanoAiu"] = json!(2 * NANO_AIU_PER_CREDIT);
    shutdown["data"]["modelMetrics"]["test-model"]["totalNanoAiu"] = json!(NANO_AIU_PER_CREDIT);
    if let Some(watermarks) = watermarks {
        checkpoint["data"]["usageAccountingWatermarks"] = watermarks.clone();
        shutdown["data"]["usageAccountingWatermarks"] = watermarks;
    }
    vec![checkpoint.to_string(), shutdown.to_string()]
}

#[test]
fn a_shutdown_counts_no_call_that_another_sessions_usage_record_reports() {
    let record = |id: &str| {
        usage_record(
            id,
            "2026-01-02T10:00:00.000Z",
            json!({"accounting": {"sourceSessionId": "session-child", "sequence": 1}}),
        )
    };
    let watermarks = Some(json!({"session-child": 1}));
    let with_copy = [
        vec![record("parent-record")],
        parent_snapshots(watermarks.clone()),
    ]
    .concat();
    let cases = [
        (
            "parent holds a copy",
            with_copy.clone(),
            true,
            vec![("session-child", "test-model", 100)],
        ),
        (
            "parent holds only its shutdown",
            parent_snapshots(watermarks),
            true,
            vec![("session-child", "test-model", 100)],
        ),
        (
            "child file missing",
            with_copy,
            false,
            vec![("session-parent", "test-model", 100)],
        ),
        (
            "parent shutdown predates usage records",
            parent_snapshots(None),
            true,
            vec![
                ("session-child", "test-model", 100),
                ("session-parent", "test-model", 100),
                ("session-parent", "unknown", 0),
            ],
        ),
    ];
    for (layout, parent, child_file, expected) in cases {
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-parent/events.jsonl": parent.join("\n"),
        });
        if child_file {
            fixture.write_file(
                "home/.copilot/session-state/session-child/events.jsonl",
                record("child-record"),
            );
        }

        let entries = load_copilot_home(&fixture, &utc_args());

        assert_eq!(
            entries
                .iter()
                .map(|entry| (
                    entry.session_id.to_string(),
                    entry.model.clone().unwrap_or_default(),
                    entry.usage.input_tokens
                ))
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|(session, model, input)| (session.to_string(), model.to_string(), *input))
                .collect::<Vec<_>>(),
            "{layout}"
        );
    }
}

fn otel_call_with(span_id: &str, timestamp: &str, attribute: (&str, &str)) -> String {
    let mut span: serde_json::Value = serde_json::from_str(&otel_call(span_id, timestamp)).unwrap();
    span["attributes"][attribute.0] = json!(attribute.1);
    span.to_string()
}

#[test]
fn counts_an_otel_call_exported_to_two_files_once() {
    let span = otel_call_with(
        "span-1",
        "2026-01-02T09:00:00.000Z",
        ("gen_ai.response.id", "response-1"),
    );
    for record in [None, Some("response-1")] {
        let fixture = fs_fixture!({
            "home/.copilot/otel/a.jsonl": span.clone(),
            "home/.copilot/otel/b.jsonl": span.clone(),
        });
        if let Some(api_call_id) = record {
            fixture.write_file(
                "home/.copilot/session-state/session-1/events.jsonl",
                usage_record(
                    "record-1",
                    "2026-01-02T09:00:00.000Z",
                    json!({"apiCallId": api_call_id}),
                ),
            );
        }

        let entries = load_copilot_home(&fixture, &utc_args());

        let expected_input = if record.is_some() { 100 } else { 1 };
        assert_eq!(
            usage_rows(&entries),
            [("2026-01-02T09:00:00.000Z".to_string(), expected_input)],
            "usage record: {record:?}"
        );
    }
}

#[test]
fn counts_an_otel_call_copied_to_another_line_of_another_file_once() {
    let span = otel_call("span-1", "2026-01-02T09:00:00.000Z");
    let fixture = fs_fixture!({
        "home/.copilot/otel/a.jsonl": span.clone(),
        "home/.copilot/otel/b.jsonl":
            [otel_call("span-2", "2026-01-02T08:00:00.000Z"), span].join("\n"),
    });

    let entries = load_copilot_home(&fixture, &utc_args());

    assert_eq!(
        usage_rows(&entries),
        [
            ("2026-01-02T08:00:00.000Z".to_string(), 1),
            ("2026-01-02T09:00:00.000Z".to_string(), 1),
        ]
    );
}

fn joined_and_split_otel_rows(lines: &[String]) -> [Vec<(String, u64)>; 2] {
    let joined = fs_fixture!({ "home/.copilot/otel/a.jsonl": lines.join("\n") });
    let split = fs_fixture!({});
    for (index, line) in lines.iter().enumerate() {
        split.write_file(format!("home/.copilot/otel/{index}.jsonl"), line);
    }
    [joined, split].map(|fixture| usage_rows(&load_copilot_home(&fixture, &utc_args())))
}

#[test]
fn counts_an_exact_otel_span_identity_once_in_one_file_or_several() {
    let span = otel_call("span-1", "2026-01-02T09:00:00.000Z");

    let [joined, split] = joined_and_split_otel_rows(&[span.clone(), span]);

    let once = [("2026-01-02T09:00:00.000Z".to_string(), 1)];
    assert_eq!(joined, once);
    assert_eq!(split, once);
}

#[test]
fn counts_a_re_exported_chat_span_once_in_one_file_or_several() {
    let response = ("gen_ai.response.id", "response-1");
    let lines = [
        otel_call_with("span-1", "2026-01-02T09:00:00.000Z", response),
        otel_call_with("span-2", "2026-01-02T09:05:00.000Z", response),
    ];

    let [joined, split] = joined_and_split_otel_rows(&lines);

    let earliest = [("2026-01-02T09:00:00.000Z".to_string(), 1)];
    assert_eq!(joined, earliest);
    assert_eq!(split, earliest);
}

#[test]
fn keeps_the_earliest_copy_of_one_span_identity_in_one_file_or_several() {
    let lines = [
        otel_call("span-1", "2026-01-02T09:05:00.000Z"),
        otel_call("span-1", "2026-01-02T09:00:00.000Z"),
    ];

    let [joined, split] = joined_and_split_otel_rows(&lines);

    let earliest = [("2026-01-02T09:00:00.000Z".to_string(), 1)];
    assert_eq!(joined, earliest);
    assert_eq!(split, earliest);
}

#[test]
fn a_re_export_inherits_its_trace_model_from_another_file() {
    let response = ("gen_ai.response.id", "response-1");
    let with_model = otel_call_with("span-1", "2026-01-02T09:00:00.000Z", response);
    let mut without_model: serde_json::Value = serde_json::from_str(&otel_call_with(
        "span-2",
        "2026-01-02T09:05:00.000Z",
        response,
    ))
    .unwrap();
    without_model["attributes"]
        .as_object_mut()
        .unwrap()
        .remove("gen_ai.response.model");
    let without_model = without_model.to_string();

    for lines in [
        [with_model.clone(), without_model.clone()],
        [without_model.clone(), with_model.clone()],
    ] {
        let [joined, split] = joined_and_split_otel_rows(&lines);

        let earliest = [("2026-01-02T09:00:00.000Z".to_string(), 1)];
        assert_eq!(joined, earliest);
        assert_eq!(split, earliest);
    }
}

#[test]
fn a_kept_span_copy_takes_the_request_ids_of_a_later_copy() {
    let unattributed = otel_call("span-1", "2026-01-02T09:00:00.000Z");
    let attributed = otel_call_with(
        "span-1",
        "2026-01-02T09:05:00.000Z",
        ("github.copilot.service_request_id", "request-1"),
    );
    let receipt = usage_record(
        "record-1",
        "2026-01-02T09:00:00.000Z",
        json!({"serviceRequestId": "request-1"}),
    );
    for split in [false, true] {
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-1/events.jsonl": receipt.clone(),
        });
        if split {
            fixture.write_file("home/.copilot/otel/0.jsonl", &unattributed);
            fixture.write_file("home/.copilot/otel/1.jsonl", &attributed);
        } else {
            fixture.write_file(
                "home/.copilot/otel/0.jsonl",
                [unattributed.clone(), attributed.clone()].join("\n"),
            );
        }

        let entries = load_copilot_home(&fixture, &utc_args());

        assert_eq!(
            usage_rows(&entries),
            [("2026-01-02T09:00:00.000Z".to_string(), 100)],
            "split={split}"
        );
    }
}

#[test]
fn keeps_distinct_service_requests_of_one_response_in_one_file_or_several() {
    let call = |span_id: &str, request_id: &str| {
        let mut span: serde_json::Value = serde_json::from_str(&otel_call_with(
            span_id,
            "2026-01-02T09:00:00.000Z",
            ("gen_ai.response.id", "turn-response"),
        ))
        .unwrap();
        span["attributes"]["github.copilot.service_request_id"] = json!(request_id);
        span.to_string()
    };

    let [joined, split] =
        joined_and_split_otel_rows(&[call("span-1", "request-1"), call("span-2", "request-2")]);

    let both = [
        ("2026-01-02T09:00:00.000Z".to_string(), 1),
        ("2026-01-02T09:00:00.000Z".to_string(), 1),
    ];
    assert_eq!(joined, both);
    assert_eq!(split, both);
}

#[test]
fn keeps_distinct_calls_of_one_response_id_in_one_file_or_several() {
    let call = |span_id: &str, input_tokens: u64| {
        let mut span: serde_json::Value =
            serde_json::from_str(&otel_call(span_id, "2026-01-02T09:00:00.000Z")).unwrap();
        span["attributes"]["gen_ai.response.id"] = json!("turn-response");
        span["attributes"]["gen_ai.usage.input_tokens"] = json!(input_tokens);
        span.to_string()
    };

    let [joined, split] = joined_and_split_otel_rows(&[call("span-1", 1), call("span-2", 2)]);

    let both = [
        ("2026-01-02T09:00:00.000Z".to_string(), 1),
        ("2026-01-02T09:00:00.000Z".to_string(), 2),
    ];
    assert_eq!(joined, both);
    assert_eq!(split, both);
}

#[test]
fn keeps_otel_calls_without_trace_and_span_ids_from_every_file() {
    let call = |input_tokens: u64| {
        let mut span: serde_json::Value =
            serde_json::from_str(&otel_call("unused", "2026-01-02T09:00:00.000Z")).unwrap();
        let span_object = span.as_object_mut().unwrap();
        span_object.remove("traceId");
        span_object.remove("spanId");
        span["attributes"]["gen_ai.usage.input_tokens"] = json!(input_tokens);
        span.to_string()
    };
    let fixture = fs_fixture!({
        "home/.copilot/otel/a.jsonl": call(1),
        "home/.copilot/otel/b.jsonl": call(2),
    });

    let entries = load_copilot_home(&fixture, &utc_args());

    assert_eq!(
        usage_rows(&entries),
        [
            ("2026-01-02T09:00:00.000Z".to_string(), 1),
            ("2026-01-02T09:00:00.000Z".to_string(), 2),
        ]
    );
}

#[test]
fn matches_call_ids_only_within_the_same_kind() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": usage_record(
            "record-1",
            "2026-01-02T09:00:00.000Z",
            json!({"apiCallId": "same-value"}),
        ),
        "home/.copilot/otel/otel.jsonl": otel_call_with(
            "span-1",
            "2026-01-02T09:00:00.000Z",
            ("github.copilot.service_request_id", "same-value"),
        ),
    });

    let entries = load_copilot_home(&fixture, &utc_args());

    assert_eq!(
        usage_rows(&entries),
        [
            ("2026-01-02T09:00:00.000Z".to_string(), 100),
            ("2026-01-02T09:00:00.000Z".to_string(), 1),
        ]
    );
}

#[test]
fn a_usage_record_replaces_one_otel_call_of_a_shared_response_id() {
    let call = |span_id: &str, timestamp: &str, input_tokens: u64| {
        let mut span: serde_json::Value =
            serde_json::from_str(&otel_call(span_id, timestamp)).unwrap();
        span["attributes"]["gen_ai.response.id"] = json!("turn-response");
        span["attributes"]["gen_ai.usage.input_tokens"] = json!(input_tokens);
        span.to_string()
    };
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": usage_record(
            "record-1",
            "2026-01-02T09:09:00.000Z",
            json!({"apiCallId": "turn-response"}),
        ),
        "home/.copilot/otel/otel.jsonl": [
            call("first", "2026-01-02T09:00:00.000Z", 1),
            call("recorded", "2026-01-02T09:10:00.000Z", 2),
        ]
        .join("\n"),
    });

    let entries = load_copilot_home(&fixture, &utc_args());

    assert_eq!(
        usage_rows(&entries),
        [
            ("2026-01-02T09:00:00.000Z".to_string(), 1),
            ("2026-01-02T09:09:00.000Z".to_string(), 100),
        ]
    );
}

#[test]
fn matches_service_request_ids_before_shared_response_ids() {
    let record = |id: &str, timestamp: &str, service_request_id: &str| {
        usage_record(
            id,
            timestamp,
            json!({"apiCallId": "turn-response", "serviceRequestId": service_request_id}),
        )
    };
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": [
            record("record-a", "2026-01-02T09:10:00.000Z", "service-a"),
            record("record-b", "2026-01-02T09:20:00.000Z", "service-b"),
        ]
        .join("\n"),
        "home/.copilot/otel/otel.jsonl": [
            otel_call_with(
                "call-a",
                "2026-01-02T09:05:00.000Z",
                ("github.copilot.service_request_id", "service-a"),
            ),
            otel_call_with(
                "call-b",
                "2026-01-02T09:11:00.000Z",
                ("gen_ai.response.id", "turn-response"),
            ),
        ]
        .join("\n"),
    });

    let entries = load_copilot_home(&fixture, &utc_args());

    assert_eq!(
        usage_rows(&entries),
        [
            ("2026-01-02T09:10:00.000Z".to_string(), 100),
            ("2026-01-02T09:20:00.000Z".to_string(), 100),
        ]
    );
}

#[test]
fn a_usage_record_claims_only_the_shutdown_covered_call_its_service_request_names() {
    let call = |span_id: &str, timestamp: &str, session: &str, input_tokens: u64| {
        let mut span: serde_json::Value =
            serde_json::from_str(&otel_call(span_id, timestamp).replace("session-1", session))
                .unwrap();
        span["attributes"]["gen_ai.response.id"] = json!("turn-response");
        span["attributes"]["gen_ai.usage.input_tokens"] = json!(input_tokens);
        span
    };
    let mut named = call("named", "2026-01-02T10:00:00.000Z", "session-parent", 1);
    named["attributes"]["github.copilot.service_request_id"] = json!("service-a");
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-parent/events.jsonl":
            shutdown_with_requests("shutdown-1", "2026-01-02T11:00:00.000Z", 1),
        "home/.copilot/session-state/session-child/events.jsonl": usage_record(
            "record-a",
            "2026-01-02T10:00:00.000Z",
            json!({"apiCallId": "turn-response", "serviceRequestId": "service-a"}),
        ),
        "home/.copilot/otel/otel.jsonl": [
            named,
            call("sibling", "2026-01-02T10:05:00.000Z", "session-child", 2),
        ]
        .map(|span| span.to_string())
        .join("\n"),
    });

    let entries = load_copilot_home(&fixture, &utc_args());

    assert_eq!(
        usage_rows(&entries),
        [
            ("2026-01-02T10:00:00.000Z".to_string(), 100),
            ("2026-01-02T10:05:00.000Z".to_string(), 2),
            ("2026-01-02T11:00:00.000Z".to_string(), 100),
        ]
    );
}

#[test]
fn keeps_otel_calls_that_no_usage_record_covers() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": usage_record(
            "record-1",
            "2026-01-02T10:00:00.000Z",
            json!({"apiCallId": "response-2"}),
        ),
        "home/.copilot/otel/otel.jsonl":
            otel_call("uncovered", "2026-01-02T09:00:00.000Z"),
    });

    let entries = load_copilot_home(&fixture, &utc_args());

    assert_eq!(
        usage_rows(&entries),
        [
            ("2026-01-02T09:00:00.000Z".to_string(), 1),
            ("2026-01-02T10:00:00.000Z".to_string(), 100),
        ]
    );
}

#[test]
fn leaves_otel_calls_that_usage_records_cover_to_the_records() {
    let otel = |span_id: &str, call_id: (&str, &str)| {
        let mut span: serde_json::Value = serde_json::from_str(
            &otel_call(span_id, "2026-02-05T09:05:00.000Z")
                .replace("session-1", "session-receipts")
                .replace("test-model", "gpt-5.4"),
        )
        .unwrap();
        span["attributes"][call_id.0] = json!(call_id.1);
        span.to_string()
    };
    for (attribute, recorded_id) in [
        ("gen_ai.response.id", "chatcmpl-r-2"),
        ("github.copilot.service_request_id", "service-r-2"),
    ] {
        let fixture = fs_fixture!({
            "home/.copilot/session-state/session-receipts/events.jsonl": RECEIPTS_ONLY,
            "home/.copilot/otel/otel.jsonl": [
                otel("recorded", (attribute, recorded_id)),
                otel("unrecorded", (attribute, "unrecorded")),
            ]
            .join("\n"),
        });

        let entries = load_copilot_home(&fixture, &utc_args());

        assert_eq!(
            token_rows(&entries)
                .into_iter()
                .filter(|row| row.1 == "gpt-5.4")
                .map(|row| (row.0, row.2))
                .collect::<Vec<_>>(),
            [
                ("2026-02-05T09:05:00.000Z".to_string(), 100),
                ("2026-02-05T09:05:00.000Z".to_string(), 1),
            ],
            "{attribute}"
        );
    }
}

fn test_model_pricing() -> ccusage_core::PricingMap {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = ccusage_core::PricingMap::default();
    pricing.load_json(
        r#"{"test-model":{"input_cost_per_token":1,"output_cost_per_token":2,"cache_creation_input_token_cost":3,"cache_read_input_token_cost":4}}"#,
    );
    pricing
}

fn costs_in_credits(entries: &[LoadedEntry]) -> Vec<u64> {
    entries
        .iter()
        .map(|entry| (entry.cost * 100.0).round() as u64)
        .collect()
}

#[test]
fn prices_a_usage_record_from_the_credits_it_recorded_per_cost_mode() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": usage_record(
            "record-1",
            "2026-02-05T09:00:00.000Z",
            json!({
                "outputTokens": 10,
                "cost": 1.0,
                "accounting": {"sourceSessionId": "session-1", "sequence": 1},
                "copilotUsage": {
                    "tokenDetails": [
                        {"tokenType": "input", "tokenCount": 100, "batchSize": 1_000_000, "costPerBatch": 25_000_000_000_u64},
                        {"tokenType": "output", "tokenCount": 10, "batchSize": 1_000_000, "costPerBatch": 250_000_000_000_u64}
                    ],
                    "totalNanoAiu": 5 * NANO_AIU_PER_CREDIT
                }
            }),
        ),
    });
    for (mode, credits) in [
        (CostMode::Auto, 5),
        (CostMode::Calculate, 12_000),
        (CostMode::Display, 5),
    ] {
        let shared = crate::cli::SharedArgs { mode, ..utc_args() };

        let entries = load_priced_copilot_home(&fixture, &shared, &test_model_pricing());

        assert_eq!(costs_in_credits(&entries), [credits], "{mode:?}");
        assert_eq!(entries[0].missing_pricing_model, None, "{mode:?}");
    }
}

#[test]
fn prices_a_usage_record_without_recorded_credits_from_its_tokens() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": usage_record(
            "record-1",
            "2026-02-05T09:00:00.000Z",
            json!({"outputTokens": 10, "cost": 1.0}),
        ),
    });

    let entries = load_priced_copilot_home(&fixture, &utc_args(), &test_model_pricing());

    assert_eq!(costs_in_credits(&entries), [12_000]);
}

fn shutdown_with_credits(id: &str, timestamp: &str, input: u64, credits: Option<u64>) -> String {
    let mut model = json!({"usage": {"inputTokens": input}, "requests": {"count": 1}});
    if let Some(credits) = credits {
        model["totalNanoAiu"] = json!(credits * NANO_AIU_PER_CREDIT);
    }
    json!({
        "type": "session.shutdown",
        "id": id,
        "timestamp": timestamp,
        "data": {"modelMetrics": {"test-model": model}}
    })
    .to_string()
}

#[test]
fn prices_each_shutdown_interval_from_the_credits_it_adds() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": [
            shutdown_with_credits("shutdown-1", "2026-01-02T10:00:00.000Z", 100, Some(10)),
            shutdown_with_credits("shutdown-2", "2026-01-03T10:00:00.000Z", 300, Some(25)),
        ]
        .join("\n"),
    });
    for (mode, credits) in [
        (CostMode::Auto, [10, 15]),
        (CostMode::Calculate, [10_000, 20_000]),
        (CostMode::Display, [10, 15]),
    ] {
        let shared = crate::cli::SharedArgs { mode, ..utc_args() };

        let entries = load_priced_copilot_home(&fixture, &shared, &test_model_pricing());

        assert_eq!(costs_in_credits(&entries), credits, "{mode:?}");
    }
}

#[test]
fn prices_a_shutdown_interval_from_its_tokens_when_either_snapshot_lacks_credits() {
    let fixture = fs_fixture!({
        "home/.copilot/session-state/session-1/events.jsonl": [
            shutdown_with_credits("shutdown-1", "2026-01-02T10:00:00.000Z", 100, None),
            shutdown_with_credits("shutdown-2", "2026-01-03T10:00:00.000Z", 300, Some(25)),
            shutdown_with_credits("shutdown-3", "2026-01-04T10:00:00.000Z", 600, Some(40)),
        ]
        .join("\n"),
    });

    let entries = load_priced_copilot_home(&fixture, &utc_args(), &test_model_pricing());

    assert_eq!(costs_in_credits(&entries), [10_000, 20_000, 15]);
}

#[cfg(unix)]
#[test]
fn names_the_io_error_kind_of_an_unreadable_source_file() {
    use std::{fs, os::unix::fs::PermissionsExt};

    let fixture = fs_fixture!({ "otel/locked.jsonl": "{}\n" });
    let locked = fixture.path("otel/locked.jsonl");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&locked).is_ok() {
        return;
    }
    Environment::with_vars([
        (
            super::super::paths::COPILOT_HOME_ENV,
            Some(OsString::from(fixture.root())),
        ),
        (
            super::super::paths::COPILOT_OTEL_FILE_EXPORTER_PATH_ENV,
            None,
        ),
    ])
    .run(|| {
        let shared = crate::cli::SharedArgs {
            debug: true,
            single_thread: true,
            ..crate::cli::SharedArgs::default()
        };
        let window = ReportWindow::from_shared(&shared);
        let diagnostics = Diagnostics::new("copilot");
        let pricing = ccusage_core::PricingMap::default();
        let cx = LoadContext {
            shared: &shared,
            kind: AgentReportKind::Daily,
            window: &window,
            pricing: ccusage_adapter_common::source::ReportPricing::Supplied(&pricing),
            options: Default::default(),
            diagnostics: &diagnostics,
        };

        let entries = load_entries_for_context(&cx).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o600)).unwrap();

        assert!(entries.is_empty());
        assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 1);
        assert!(
            diagnostics
                .warnings(&shared)
                .iter()
                .any(|line| line.contains("I/O error (PermissionDenied)")),
            "{:?}",
            diagnostics.warnings(&shared)
        );
    })
}
