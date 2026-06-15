use std::{collections::HashMap, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;

use super::{
    parser::{CodebuffEntry, calculate_codebuff_cost, load_chat_file, missing_codebuff_pricing},
    paths::discover_chat_files,
};
use crate::{
    LoadedEntry, PricingMap, Result, UsageEntry, UsageMessage, cli::SharedArgs, debug_log,
    format_date_tz, parse_tz, read_files_parallel,
};

pub(crate) fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent::Codebuff,
        shared.json,
        || load_entries_inner(shared, pricing),
    )
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let mut files = discover_chat_files()?;
    files.sort();
    // Read files in parallel but apply the last-wins dedup sequentially over the
    // original (sorted) file order, so the surviving entry per dedup key is
    // identical to the single-threaded read.
    let loaded = read_files_parallel(&files, shared.single_thread, |file| {
        load_chat_file(file).unwrap_or_else(|error| {
            debug_log(
                shared,
                format!(
                    "Failed to read Codebuff chat file {}: {error}",
                    file.display()
                ),
            );
            Vec::new()
        })
    });
    let mut deduped = HashMap::<String, CodebuffEntry>::new();
    for file_entries in loaded {
        for entry in file_entries {
            deduped.insert(entry.dedup_key.clone(), entry);
        }
    }
    let mut entries = deduped
        .into_values()
        .map(|entry| to_loaded_entry(entry, tz.as_ref(), pricing))
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

fn to_loaded_entry(
    entry: CodebuffEntry,
    tz: Option<&JiffTimeZone>,
    pricing: &PricingMap,
) -> LoadedEntry {
    let cost = calculate_codebuff_cost(&entry, pricing);
    let missing_pricing_model = missing_codebuff_pricing(&entry, pricing);
    let data = UsageEntry {
        session_id: Some(entry.session_id.clone()),
        timestamp: entry.timestamp_text.clone(),
        version: None,
        message: UsageMessage {
            usage: entry.usage,
            model: Some(entry.model.clone()),
            id: Some(entry.dedup_key.clone()),
        },
        cost_usd: None,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };
    LoadedEntry {
        date: format_date_tz(entry.timestamp, tz),
        timestamp: entry.timestamp,
        project: Arc::from("codebuff"),
        session_id: Arc::from(entry.session_id.as_str()),
        project_path: Arc::from("Codebuff"),
        cost,
        extra_total_tokens: entry.extra_total_tokens,
        credits: (entry.credits > 0.0).then_some(entry.credits),
        model: Some(entry.model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        message_count: None,
        data,
    }
}

#[cfg(test)]
use super::report::{report_from_rows, summarize_entries};

#[cfg(test)]
mod tests {
    use super::super::{parser::parse_usage_object, paths::CODEBUFF_DATA_DIR_ENV};
    use super::*;
    use crate::{
        TokenUsageRaw, UsageEntry, UsageMessage, cli::AgentReportKind, parse_ts_timestamp,
    };
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    #[test]
    fn loads_assistant_usage_from_chat_messages() {
        let fixture = fs_fixture!({
            "projects/project-a/chats/2026-01-02T03-04-05.000Z/chat-messages.json": r#"[
                {"role":"user","text":"hello"},
                {"id":"assistant-message","role":"assistant","timestamp":"2026-01-02T03:04:06.000Z","metadata":{"model":"claude-sonnet-4-20250514","usage":{"inputTokens":100,"outputTokens":50,"cacheCreationInputTokens":20,"cacheReadInputTokens":10}},"credits":1.25}
            ]"#,
        });
        let _cleanup = EnvVarGuard::set(CODEBUFF_DATA_DIR_ENV, fixture.root());

        let pricing = PricingMap::load_embedded();
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries(&shared, &pricing).unwrap();

        let channel = fixture.root().file_name().unwrap().to_str().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2026-01-02");
        assert_eq!(
            entries[0].session_id.as_ref(),
            format!("{channel}/project-a/2026-01-02T03-04-05.000Z")
        );
        assert_eq!(entries[0].data.message.usage.input_tokens, 100);
        assert_eq!(entries[0].data.message.usage.output_tokens, 50);
        assert_eq!(
            entries[0].data.message.usage.cache_creation_input_tokens,
            20
        );
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 10);
        assert_eq!(entries[0].credits, Some(1.25));
    }

    #[test]
    fn falls_back_to_run_state_provider_usage() {
        let fixture = fs_fixture!({
            "projects/project-a/chats/2026-01-02T03-04-05.000Z/chat-messages.json": r#"[
                {"variant":"agent","metadata":{"runState":{"sessionState":{"mainAgentState":{"messageHistory":[
                    {"role":"user","providerOptions":{}},
                    {"role":"assistant","providerOptions":{"codebuff":{"model":"openai/gpt-5","usage":{"prompt_tokens":100,"completion_tokens":50,"prompt_tokens_details":{"cached_tokens":10}}}}}
                ]}}}}}
            ]"#,
        });
        let _cleanup = EnvVarGuard::set(CODEBUFF_DATA_DIR_ENV, fixture.root());

        let pricing = PricingMap::load_embedded();
        let entries = load_entries(&SharedArgs::default(), &pricing).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].data.message.model.as_deref(),
            Some("openai/gpt-5")
        );
        assert_eq!(entries[0].data.message.usage.input_tokens, 100);
        assert_eq!(entries[0].data.message.usage.output_tokens, 50);
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 10);
    }

    #[test]
    fn falls_back_to_total_tokens_when_codebuff_parts_are_missing() {
        let usage = parse_usage_object(Some(&serde_json::json!({
            "totalTokens": 789
        })));

        assert_eq!(usage.output_tokens, 789);
        assert_eq!(usage.extra_total_tokens, 0);
    }

    #[test]
    fn report_includes_credits() {
        let entry = LoadedEntry {
            data: UsageEntry {
                session_id: Some("session-a".to_string()),
                timestamp: "2026-01-02T03:04:06.000Z".to_string(),
                version: None,
                message: UsageMessage {
                    usage: TokenUsageRaw {
                        input_tokens: 100,
                        output_tokens: 50,
                        cache_creation_input_tokens: 20,
                        cache_read_input_tokens: 10,
                        speed: None,
                        cache_creation: None,
                    },
                    model: Some("claude-sonnet-4-20250514".to_string()),
                    id: Some("message-a".to_string()),
                },
                cost_usd: None,
                request_id: None,
                is_api_error_message: None,
                is_sidechain: None,
            },
            timestamp: parse_ts_timestamp("2026-01-02T03:04:06.000Z").unwrap(),
            date: "2026-01-02".to_string(),
            project: Arc::from("codebuff"),
            session_id: Arc::from("session-a"),
            project_path: Arc::from("Codebuff"),
            cost: 0.02,
            extra_total_tokens: 0,
            credits: Some(1.25),
            model: Some("claude-sonnet-4-20250514".to_string()),
            usage_limit_reset_time: None,
            missing_pricing_model: None,
            message_count: None,
        };
        let rows = summarize_entries(&[entry], AgentReportKind::Daily).unwrap();
        let report = report_from_rows(&rows, AgentReportKind::Daily);

        assert_eq!(report["daily"][0]["credits"], serde_json::json!(1.25));
        assert_eq!(report["totals"]["credits"], serde_json::json!(1.25));
    }
}
