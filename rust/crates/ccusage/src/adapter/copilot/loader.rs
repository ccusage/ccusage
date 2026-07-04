use std::{path::Path, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;

use super::{
    parser::{CopilotUsageEntry, parse_otel_file},
    paths::paths,
};
use crate::{
    LoadedEntry, Result, TokenUsageRaw, UsageEntry, UsageMessage, calculate_cost_for_usage,
    cli::CostMode, debug_log, format_date_tz, missing_pricing_model_for_usage, parse_tz,
    read_files_parallel,
};

pub(crate) fn load_entries(
    shared: &crate::cli::SharedArgs,
    pricing: &crate::PricingMap,
) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent::Copilot,
        shared.json,
        || load_entries_inner(shared, pricing),
    )
}

fn load_entries_inner(
    shared: &crate::cli::SharedArgs,
    pricing: &crate::PricingMap,
) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let files = paths()?;
    // Read OTEL files in parallel; entries keep their original file order before
    // the stable sort, so output is identical to the sequential read.
    let loaded = read_files_parallel(&files, shared.single_thread, |path| {
        read_otel_file(path, tz.as_ref(), shared.mode, pricing).unwrap_or_else(|error| {
            debug_log(
                shared,
                format!(
                    "Failed to read Copilot OTEL file {}: {error}",
                    path.display()
                ),
            );
            Vec::new()
        })
    });
    let mut entries = Vec::new();
    for file_entries in loaded {
        entries.extend(file_entries);
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

fn read_otel_file(
    path: &Path,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: &crate::PricingMap,
) -> Result<Vec<LoadedEntry>> {
    Ok(parse_otel_file(path)?
        .into_iter()
        .map(|entry| usage_entry_to_loaded(entry, tz, mode, pricing))
        .collect())
}

fn usage_entry_to_loaded(
    entry: CopilotUsageEntry,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: &crate::PricingMap,
) -> LoadedEntry {
    let usage = TokenUsageRaw {
        input_tokens: entry.input_tokens,
        output_tokens: entry.output_tokens,
        cache_creation_input_tokens: entry.cache_creation_tokens,
        cache_read_input_tokens: entry.cache_read_tokens,
        speed: None,
        cache_creation: None,
    };
    let cost_usage = TokenUsageRaw {
        output_tokens: entry.output_tokens + entry.reasoning_output_tokens,
        cache_creation: None,
        ..usage
    };
    let data = UsageEntry {
        session_id: Some(entry.session_id.clone()),
        timestamp: entry.timestamp_text,
        version: None,
        message: UsageMessage {
            usage,
            model: Some(entry.model.clone()),
            id: Some(entry.dedup_key),
        },
        cost_usd: None,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };
    let cost = calculate_cost_for_usage(Some(&entry.model), cost_usage, None, mode, Some(pricing));
    let missing_pricing_model =
        missing_pricing_model_for_usage(Some(&entry.model), cost_usage, None, mode, Some(pricing));
    LoadedEntry {
        date: format_date_tz(entry.timestamp, tz),
        timestamp: entry.timestamp,
        project: Arc::from("copilot"),
        session_id: Arc::from(entry.session_id),
        project_path: Arc::from("GitHub Copilot CLI"),
        cost,
        extra_total_tokens: entry.reasoning_output_tokens,
        credits: None,
        message_count: None,
        model: Some(entry.model),
        data,
        usage_limit_reset_time: None,
        missing_pricing_model,
    }
}

#[cfg(test)]
use super::report::{report_from_rows, summarize_entries};

#[cfg(test)]
mod tests {
    use ccusage_test_support::fs_fixture;
    use serde_json::json;

    use super::super::parser::parse_otel_file;
    use super::*;
    use crate::cli::AgentReportKind;

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

        let entries = parse_otel_file(&file).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].timestamp_text, "2026-04-11T19:04:24.967Z");
        assert_eq!(entries[0].session_id, "conv-1");
        assert_eq!(entries[0].model, "claude-sonnet-4");
        assert_eq!(entries[0].input_tokens, 19_329);
        assert_eq!(entries[0].output_tokens, 281);
        assert_eq!(entries[0].cache_creation_tokens, 25);
        assert_eq!(entries[0].cache_read_tokens, 123);
        assert_eq!(entries[0].reasoning_output_tokens, 128);
        assert_eq!(entries[0].dedup_key, "trace-1:span-1");
    }

    #[test]
    fn suppresses_lower_priority_records_for_same_response() {
        let fixture = fs_fixture!({
            "copilot.jsonl": [
                json!({
                    "type": "span",
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

        let entries = parse_otel_file(&file).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].dedup_key, "trace-dupe:chat-1");
        assert_eq!(entries[0].input_tokens, 60);
        assert_eq!(entries[0].output_tokens, 10);
    }

    #[test]
    fn includes_reasoning_tokens_in_total_tokens() {
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
        let mut pricing = crate::PricingMap::default();
        pricing.load_json(
            r#"{"test-model":{"input_cost_per_token":1,"output_cost_per_token":2,"cache_creation_input_token_cost":3,"cache_read_input_token_cost":4}}"#,
        );

        let loaded = read_otel_file(&file, None, CostMode::Auto, &pricing).unwrap();
        let rows = summarize_entries(&loaded, AgentReportKind::Daily).unwrap();
        let report = report_from_rows(&rows, AgentReportKind::Daily);

        assert_eq!(report["daily"][0]["inputTokens"], 90);
        assert_eq!(report["daily"][0]["outputTokens"], 50);
        assert_eq!(report["daily"][0]["totalTokens"], 175);
        assert_eq!(report["daily"][0]["totalCost"], 300.0);
        assert_eq!(
            report["daily"][0]["modelBreakdowns"],
            json!([{
                "modelName": "test-model",
                "inputTokens": 90,
                "outputTokens": 50,
                "cacheCreationTokens": 20,
                "cacheReadTokens": 10,
                "cost": 300.0
            }])
        );
    }

    #[test]
    fn falls_back_to_total_tokens_when_copilot_parts_are_missing() {
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
                    },
                })
            ),
        });
        let file = fixture.path("copilot.jsonl");

        let entries = parse_otel_file(&file).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].output_tokens, 567);
        assert_eq!(entries[0].reasoning_output_tokens, 0);
    }
}
