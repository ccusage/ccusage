use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
    sync::Arc,
};

use jiff::tz::TimeZone as JiffTimeZone;
use serde_json::Value;

use super::{parser::parse_usage_record, paths::discover_session_files};
use crate::{
    LoadedEntry, PricingMap, Result, UsageEntry, UsageMessage, calculate_cost,
    cli::{CostMode, SharedArgs},
    debug_log, format_date_tz, missing_pricing_model_for_usage, parse_tz, read_files_parallel,
};

pub fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(crate::progress::UsageLoadAgent("GJC"), shared.json, || {
        load_entries_inner(shared, pricing)
    })
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let timezone = parse_tz(shared.timezone.as_deref());
    let files = discover_session_files()?;
    let loaded = read_files_parallel(&files, shared.single_thread, |path| {
        read_session_file(path, timezone.as_ref(), shared.mode, pricing).unwrap_or_else(|error| {
            debug_log(
                shared,
                format!(
                    "Failed to read GJC session file {}: {error}",
                    path.display()
                ),
            );
            Vec::new()
        })
    });
    Ok(loaded.into_iter().flatten().collect())
}

fn read_session_file(
    path: &Path,
    timezone: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: &PricingMap,
) -> Result<Vec<LoadedEntry>> {
    let reader = BufReader::new(File::open(path)?);
    let mut session_id = None;
    let mut project_path = None;
    let mut entries = Vec::new();

    for line in reader.lines() {
        let line = line?;
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) == Some("session") {
            session_id = value.get("id").and_then(Value::as_str).map(str::to_string);
            project_path = value.get("cwd").and_then(Value::as_str).map(str::to_string);
            continue;
        }
        let Some(usage) = parse_usage_record(&value) else {
            continue;
        };
        let Some(timestamp) = crate::parse_ts_timestamp(&usage.timestamp) else {
            continue;
        };
        let Some(session_id) = session_id.as_deref() else {
            continue;
        };
        let project_path = project_path.as_deref().unwrap_or("GJC");
        let data = UsageEntry {
            session_id: Some(session_id.to_string()),
            timestamp: usage.timestamp.clone(),
            version: None,
            message: UsageMessage {
                usage: usage.usage,
                model: Some(usage.model.clone()),
                id: Some(usage.message_id),
            },
            cost_usd: usage.cost_usd,
            request_id: None,
            is_api_error_message: None,
            is_sidechain: None,
        };
        let cost_data = UsageEntry {
            message: UsageMessage {
                usage: crate::TokenUsageRaw {
                    output_tokens: data
                        .message
                        .usage
                        .output_tokens
                        .saturating_add(usage.extra_total_tokens),
                    cache_creation: None,
                    ..data.message.usage
                },
                ..data.message.clone()
            },
            ..data.clone()
        };
        let cost = calculate_cost(&cost_data, mode, Some(pricing));
        let missing_pricing_model = missing_pricing_model_for_usage(
            Some(&usage.model),
            cost_data.message.usage,
            usage.cost_usd,
            mode,
            Some(pricing),
        );
        entries.push(LoadedEntry {
            date: format_date_tz(timestamp, timezone),
            timestamp,
            project: Arc::from("gjc"),
            session_id: Arc::from(session_id),
            project_path: Arc::from(project_path),
            cost,
            credits: None,
            extra_total_tokens: usage.extra_total_tokens,
            model: Some(usage.model),
            usage_limit_reset_time: None,
            missing_pricing_model,
            message_count: None,
            data,
        });
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    use super::super::{
        paths::GJC_CODING_AGENT_DIR_ENV,
        report::{report_from_rows, summarize_entries},
    };
    use super::*;
    use crate::cli::AgentReportKind;

    #[test]
    fn loads_and_aggregates_gjc_session_usage() {
        let fixture = fs_fixture!({
            "agent/sessions/project/session.jsonl": concat!(
                "{\"type\":\"session\",\"id\":\"session-a\",\"timestamp\":\"2026-08-28T01:00:00.000Z\",\"cwd\":\"/workspace/project\"}\n",
                "{\"type\":\"message\",\"id\":\"assistant-1\",\"timestamp\":\"2026-08-28T01:02:03.000Z\",\"message\":{\"role\":\"assistant\",\"model\":\"gpt-5.6-sol\",\"usage\":{\"input\":100,\"output\":50,\"cacheRead\":20,\"cacheWrite\":10,\"totalTokens\":180,\"cost\":{\"total\":0.25}}}}\n",
                "{\"type\":\"message\",\"id\":\"tool-1\",\"timestamp\":\"2026-08-28T01:02:04.000Z\",\"message\":{\"role\":\"toolResult\"}}\n"
            ),
        });
        let _guard = EnvVarGuard::set(GJC_CODING_AGENT_DIR_ENV, fixture.path("agent"));
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        let entries = load_entries(&shared, &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2026-08-28");
        assert_eq!(entries[0].session_id.as_ref(), "session-a");
        assert_eq!(entries[0].project_path.as_ref(), "/workspace/project");
        assert_eq!(entries[0].data.message.usage.input_tokens, 100);
        assert_eq!(entries[0].data.message.usage.output_tokens, 50);
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 20);
        assert_eq!(
            entries[0].data.message.usage.cache_creation_input_tokens,
            10
        );
        assert_eq!(entries[0].cost, 0.25);

        let rows = summarize_entries(&entries, AgentReportKind::Daily).unwrap();
        let report = report_from_rows(&rows, AgentReportKind::Daily);
        assert_eq!(report["daily"][0]["totalTokens"], 180);
        assert_eq!(report["totals"]["totalCost"], 0.25);
    }
}
