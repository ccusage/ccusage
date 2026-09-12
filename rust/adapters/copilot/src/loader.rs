use std::{collections::HashMap, path::Path, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;

use super::{
    parser::{CopilotUsageEntry, CopilotUsageKind, parse_otel_file, parse_session_state_file},
    paths::{CopilotSourceKind, paths},
};
use crate::{
    LoadedEntry, Result, TokenUsageRaw, UsageEntry, UsageMessage, calculate_cost_for_usage_at,
    cli::CostMode, date_range_bounds_ms, debug_log, format_date_tz,
    missing_pricing_model_for_usage, parse_tz, read_files_parallel,
};

pub fn load_entries(
    shared: &crate::cli::SharedArgs,
    pricing: &crate::PricingMap,
) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent("GitHub Copilot CLI"),
        shared.json,
        || load_entries_inner(shared, pricing),
    )
}

fn load_entries_inner(
    shared: &crate::cli::SharedArgs,
    pricing: &crate::PricingMap,
) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let sources = paths()?;
    let source_kinds = sources
        .iter()
        .map(|source| (source.path.clone(), source.kind))
        .collect::<HashMap<_, _>>();
    let files = sources
        .iter()
        .map(|source| source.path.clone())
        .collect::<Vec<_>>();
    // Read source files in parallel; entries keep their original file order before
    // the stable sort, so OTel-only output is identical to the previous read.
    let parsed = read_files_parallel(&files, shared.single_thread, |path| {
        let kind = source_kinds
            .get(path)
            .copied()
            .unwrap_or(CopilotSourceKind::Otel);
        read_source_file(path, kind).unwrap_or_else(|error| {
            let source_name = match kind {
                CopilotSourceKind::Otel => "OTEL",
                CopilotSourceKind::SessionState => "session-state",
            };
            debug_log(
                shared,
                format!(
                    "Failed to read Copilot {source_name} file {}: {error}",
                    path.display(),
                ),
            );
            Vec::new()
        })
    });
    let mut otel_entries = Vec::new();
    let mut session_state_entries = Vec::new();
    for (source, file_entries) in sources.iter().map(|source| source.kind).zip(parsed) {
        match source {
            CopilotSourceKind::Otel => otel_entries.extend(file_entries),
            CopilotSourceKind::SessionState => session_state_entries.extend(file_entries),
        }
    }
    let (since_millis, until_millis) = date_range_bounds_ms(
        shared.since.as_deref(),
        shared.until.as_deref(),
        tz.as_ref(),
    );
    let session_state =
        reconcile_session_state_entries(session_state_entries, since_millis, until_millis);
    let latest_shutdown_timestamps = session_state
        .shutdown_entries
        .iter()
        .map(|entry| {
            (
                (entry.session_id.as_str(), entry.model.as_str()),
                entry.timestamp,
            )
        })
        .collect::<HashMap<_, _>>();
    otel_entries.retain(|entry| {
        latest_shutdown_timestamps
            .get(&(entry.session_id.as_str(), entry.model.as_str()))
            .is_none_or(|shutdown_timestamp| entry.timestamp > *shutdown_timestamp)
    });

    let mut entries = session_state
        .entries
        .into_iter()
        .chain(otel_entries)
        .map(|entry| usage_entry_to_loaded(entry, tz.as_ref(), shared.mode, pricing))
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

struct SessionStateReconciliation {
    entries: Vec<CopilotUsageEntry>,
    shutdown_entries: Vec<CopilotUsageEntry>,
}

// Session-state usage is cumulative, so a range needs the latest visible
// snapshot and the latest snapshot before its lower bound as a subtraction baseline.
fn reconcile_session_state_entries(
    entries: Vec<CopilotUsageEntry>,
    since_millis: Option<i64>,
    until_millis: Option<i64>,
) -> SessionStateReconciliation {
    let entries = deduplicate_session_entries(entries);
    let mut latest_by_key = HashMap::<(String, String), usize>::new();
    let mut baseline_by_key = HashMap::<(String, String), usize>::new();
    for (index, entry) in entries.iter().enumerate() {
        let key = (entry.session_id.clone(), entry.model.clone());
        if until_millis.is_none_or(|end| entry.timestamp.as_millis() < end)
            && latest_by_key
                .get(&key)
                .is_none_or(|previous| entries[*previous].timestamp <= entry.timestamp)
        {
            latest_by_key.insert(key, index);
        }
        let key = (entry.session_id.clone(), entry.model.clone());
        if since_millis.is_some_and(|start| entry.timestamp.as_millis() < start)
            && baseline_by_key
                .get(&key)
                .is_none_or(|previous| entries[*previous].timestamp <= entry.timestamp)
        {
            baseline_by_key.insert(key, index);
        }
    }
    let mut latest_indices = latest_by_key.into_values().collect::<Vec<_>>();
    latest_indices.sort_unstable();
    let shutdown_entries = latest_indices
        .iter()
        .map(|index| entries[*index].clone())
        .collect();
    let entries = latest_indices
        .into_iter()
        .filter_map(|index| {
            let entry = &entries[index];
            let key = (entry.session_id.clone(), entry.model.clone());
            let reconciled = baseline_by_key.get(&key).map_or_else(
                || entry.clone(),
                |baseline| subtract_usage(entry, &entries[*baseline]),
            );
            has_usage(&reconciled).then_some(reconciled)
        })
        .collect();
    SessionStateReconciliation {
        entries,
        shutdown_entries,
    }
}

fn deduplicate_session_entries(entries: Vec<CopilotUsageEntry>) -> Vec<CopilotUsageEntry> {
    let mut indexes = HashMap::<String, usize>::new();
    for (index, entry) in entries.iter().enumerate() {
        if indexes
            .get(&entry.dedup_key)
            .is_none_or(|previous| entries[*previous].timestamp <= entry.timestamp)
        {
            indexes.insert(entry.dedup_key.clone(), index);
        }
    }
    let mut indexes = indexes.into_values().collect::<Vec<_>>();
    indexes.sort_unstable();
    indexes
        .into_iter()
        .map(|index| entries[index].clone())
        .collect()
}

fn subtract_usage(current: &CopilotUsageEntry, baseline: &CopilotUsageEntry) -> CopilotUsageEntry {
    CopilotUsageEntry {
        timestamp: current.timestamp,
        timestamp_text: current.timestamp_text.clone(),
        session_id: current.session_id.clone(),
        model: current.model.clone(),
        kind: current.kind,
        input_tokens: current.input_tokens.saturating_sub(baseline.input_tokens),
        output_tokens: current.output_tokens.saturating_sub(baseline.output_tokens),
        cache_creation_tokens: current
            .cache_creation_tokens
            .saturating_sub(baseline.cache_creation_tokens),
        cache_read_tokens: current
            .cache_read_tokens
            .saturating_sub(baseline.cache_read_tokens),
        reasoning_output_tokens: current
            .reasoning_output_tokens
            .saturating_sub(baseline.reasoning_output_tokens),
        extra_total_tokens: current
            .extra_total_tokens
            .saturating_sub(baseline.extra_total_tokens),
        request_count: current.request_count.saturating_sub(baseline.request_count),
        dedup_key: current.dedup_key.clone(),
    }
}

fn has_usage(entry: &CopilotUsageEntry) -> bool {
    entry.input_tokens > 0
        || entry.output_tokens > 0
        || entry.cache_creation_tokens > 0
        || entry.cache_read_tokens > 0
        || entry.reasoning_output_tokens > 0
        || entry.extra_total_tokens > 0
        || entry.request_count > 0
}

fn read_source_file(path: &Path, kind: CopilotSourceKind) -> Result<Vec<CopilotUsageEntry>> {
    match kind {
        CopilotSourceKind::Otel => parse_otel_file(path),
        CopilotSourceKind::SessionState => parse_session_state_file(path),
    }
}

#[cfg(test)]
fn read_otel_file(
    path: &Path,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: &crate::PricingMap,
) -> Result<Vec<LoadedEntry>> {
    Ok(read_source_file(path, CopilotSourceKind::Otel)?
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
        output_tokens: usage.output_tokens.saturating_add(entry.extra_total_tokens),
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
        attribution_plugin: None,
        attribution_skill: None,
    };
    let cost = calculate_cost_for_usage_at(
        Some(&entry.model),
        cost_usage,
        None,
        Some(entry.timestamp),
        mode,
        Some(pricing),
    );
    let missing_pricing_model =
        missing_pricing_model_for_usage(Some(&entry.model), cost_usage, None, mode, Some(pricing));
    LoadedEntry {
        date: format_date_tz(entry.timestamp, tz),
        timestamp: entry.timestamp,
        project: Arc::from("copilot"),
        session_id: Arc::from(entry.session_id),
        project_path: Arc::from("GitHub Copilot CLI"),
        cost,
        extra_total_tokens: entry.extra_total_tokens,
        credits: None,
        message_count: match entry.kind {
            CopilotUsageKind::Otel => (entry.request_count > 0).then_some(entry.request_count),
            CopilotUsageKind::SessionState => {
                (entry.request_count > 1).then_some(entry.request_count)
            }
        },
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
    use std::ffi::OsString;

    use ccusage_test_support::{EnvVarsGuard, fs_fixture};
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
    fn does_not_double_count_reasoning_tokens() {
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
        assert_eq!(report["daily"][0]["totalTokens"], 170);
        assert_eq!(report["daily"][0]["totalCost"], 290.0);
        assert_eq!(
            report["daily"][0]["modelBreakdowns"],
            json!([{
                "modelName": "test-model",
                "inputTokens": 90,
                "outputTokens": 50,
                "cacheCreationTokens": 20,
                "cacheReadTokens": 10,
                "cost": 290.0
            }])
        );
    }

    #[test]
    fn includes_separate_otel_reasoning_tokens_in_total_and_cost() {
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
                        "gen_ai.usage.total_tokens": 175,
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

        assert_eq!(loaded[0].extra_total_tokens, 5);
        assert_eq!(report["daily"][0]["outputTokens"], 50);
        assert_eq!(report["daily"][0]["totalTokens"], 175);
        assert_eq!(report["daily"][0]["totalCost"], 300.0);
        assert_eq!(report["daily"][0]["modelBreakdowns"][0]["cost"], 300.0);
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
                        "gen_ai.usage.reasoning_tokens": 5,
                    },
                })
            ),
        });
        let file = fixture.path("copilot.jsonl");

        let entries = parse_otel_file(&file).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].output_tokens, 567);
        assert_eq!(entries[0].reasoning_output_tokens, 5);
        assert_eq!(entries[0].extra_total_tokens, 0);
    }

    #[test]
    fn loads_session_state_tokens_and_calculates_token_cost() {
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
        let _guard = EnvVarsGuard::set_many([
            ("HOME", Some(OsString::from(fixture.path("home")))),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
            (super::super::paths::COPILOT_HOME_ENV, None),
            (
                super::super::paths::COPILOT_OTEL_FILE_EXPORTER_PATH_ENV,
                None,
            ),
        ]);
        let mut pricing = crate::PricingMap::default();
        pricing.load_json(
            r#"{"test-model":{"input_cost_per_token":1,"output_cost_per_token":2,"cache_creation_input_token_cost":3,"cache_read_input_token_cost":4}}"#,
        );
        let shared = crate::cli::SharedArgs {
            mode: CostMode::Auto,
            single_thread: true,
            ..crate::cli::SharedArgs::default()
        };

        let entries = load_entries_inner(&shared, &pricing).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.input_tokens, 70);
        assert_eq!(entries[0].data.message.usage.output_tokens, 50);
        assert_eq!(
            entries[0].data.message.usage.cache_creation_input_tokens,
            20
        );
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 10);
        assert_eq!(entries[0].extra_total_tokens, 0);
        assert_eq!(entries[0].message_count, Some(3));
        assert_eq!(entries[0].cost, 270.0);
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
        let _guard = EnvVarsGuard::set_many([
            ("HOME", Some(OsString::from(fixture.path("home")))),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
            (super::super::paths::COPILOT_HOME_ENV, None),
            (
                super::super::paths::COPILOT_OTEL_FILE_EXPORTER_PATH_ENV,
                None,
            ),
        ]);
        let shared = crate::cli::SharedArgs {
            single_thread: true,
            timezone: Some("UTC".to_string()),
            until: Some("20260102".to_string()),
            ..crate::cli::SharedArgs::default()
        };

        let entries = load_entries_inner(&shared, &crate::PricingMap::default()).unwrap();
        let mut entries = entries;
        ccusage_adapter_common::filter_loaded_entries_by_date(&mut entries, &shared);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].data.message.usage.input_tokens, 70);
        assert_eq!(entries[0].data.message.usage.output_tokens, 50);
        assert_eq!(entries[1].data.message.usage.input_tokens, 11);
        assert_eq!(entries[1].data.message.usage.output_tokens, 12);
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
        let _guard = EnvVarsGuard::set_many([
            ("HOME", Some(OsString::from(fixture.path("home")))),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
            (super::super::paths::COPILOT_HOME_ENV, None),
            (
                super::super::paths::COPILOT_OTEL_FILE_EXPORTER_PATH_ENV,
                None,
            ),
        ]);
        let shared = crate::cli::SharedArgs {
            single_thread: true,
            since: Some("20260103".to_string()),
            timezone: Some("UTC".to_string()),
            ..crate::cli::SharedArgs::default()
        };

        let entries = load_entries_inner(&shared, &crate::PricingMap::default()).unwrap();
        let mut entries = entries;
        ccusage_adapter_common::filter_loaded_entries_by_date(&mut entries, &shared);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].data.message.usage.input_tokens, 80);
        assert_eq!(entries[0].data.message.usage.output_tokens, 30);
        assert_eq!(
            entries[0].data.message.usage.cache_creation_input_tokens,
            10
        );
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 10);
        assert_eq!(entries[0].message_count, Some(2));
        assert_eq!(entries[1].data.message.usage.input_tokens, 13);
        assert_eq!(entries[1].data.message.usage.output_tokens, 14);
        assert_eq!(entries[1].message_count, Some(1));
    }

    #[test]
    fn normalizes_copilot_model_suffixes_for_pricing_and_otel_dedupe() {
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
        let _guard = EnvVarsGuard::set_many([
            ("HOME", Some(OsString::from(fixture.path("home")))),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
            (super::super::paths::COPILOT_HOME_ENV, None),
            (
                super::super::paths::COPILOT_OTEL_FILE_EXPORTER_PATH_ENV,
                None,
            ),
        ]);
        let shared = crate::cli::SharedArgs {
            mode: CostMode::Auto,
            single_thread: true,
            ..crate::cli::SharedArgs::default()
        };

        let entries = load_entries_inner(&shared, &crate::PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("claude-opus-4.6"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 70);
        assert_eq!(entries[0].data.message.usage.output_tokens, 50);
        assert_eq!(
            entries[0].data.message.usage.cache_creation_input_tokens,
            20
        );
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 10);
        assert_eq!(entries[0].extra_total_tokens, 0);
        assert!((entries[0].cost - 0.00173).abs() < 1e-12);
    }

    #[test]
    fn keeps_only_latest_cumulative_shutdown_and_unmatched_otel_rows() {
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
        let _guard = EnvVarsGuard::set_many([
            ("HOME", Some(OsString::from(fixture.path("home")))),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
            (super::super::paths::COPILOT_HOME_ENV, None),
            (
                super::super::paths::COPILOT_OTEL_FILE_EXPORTER_PATH_ENV,
                None,
            ),
        ]);
        let shared = crate::cli::SharedArgs {
            single_thread: true,
            ..crate::cli::SharedArgs::default()
        };

        let entries = load_entries_inner(&shared, &crate::PricingMap::default()).unwrap();
        let rows = entries
            .iter()
            .map(|entry| {
                (
                    entry.session_id.to_string(),
                    entry.model.clone().unwrap_or_default(),
                    entry.data.message.usage.input_tokens,
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            rows,
            vec![
                ("session-1".to_string(), "other-model".to_string(), 3),
                ("session-2".to_string(), "test-model".to_string(), 5),
                ("session-1".to_string(), "test-model".to_string(), 30),
                ("session-1".to_string(), "test-model".to_string(), 7),
            ]
        );
    }
}
