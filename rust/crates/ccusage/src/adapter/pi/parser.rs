use std::{fs, path::Path, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;
use serde_json::Value;

use crate::{
    LoadedEntry, PricingMap, Result, TokenUsageRaw, UsageEntry, UsageMessage,
    apply_total_token_fallback, calculate_cost_for_usage,
    cli::CostMode,
    fast::{LinePrefilter, prefiltered_json_values},
    format_date_tz, json_value_u64, missing_pricing_model_for_usage, non_empty_json_string,
};

pub(crate) fn read_session_file(
    path: &Path,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> Result<Vec<LoadedEntry>> {
    let content = fs::read(path)?;
    let project = extract_project(path);
    let session_id = extract_session_id(path);
    let mut entries = Vec::new();

    let prefilter = LinePrefilter::all(&[br#""usage""#, br#""message""#]);
    for value in prefiltered_json_values(&content, &prefilter) {
        if !is_pi_message_usage(&value) {
            continue;
        }
        let Some(timestamp_text) = non_empty_json_string(value.get("timestamp")) else {
            continue;
        };
        let Some(timestamp) = crate::parse_ts_timestamp(&timestamp_text) else {
            continue;
        };
        let Some(message) = value.get("message") else {
            continue;
        };
        let Some(usage_value) = message.get("usage") else {
            continue;
        };
        let input = json_value_u64(usage_value.get("input"));
        let output = json_value_u64(usage_value.get("output"));
        let cache_read = json_value_u64(usage_value.get("cacheRead"));
        let cache_create = json_value_u64(usage_value.get("cacheWrite"));
        let total = json_value_u64(usage_value.get("totalTokens"));
        let usage = TokenUsageRaw {
            input_tokens: input,
            output_tokens: output,
            cache_creation_input_tokens: cache_create,
            cache_read_input_tokens: cache_read,
            speed: None,
            cache_creation: None,
        };
        let (usage, extra_total_tokens) = apply_total_token_fallback(usage, 0, total);
        if crate::total_usage_tokens(usage) + extra_total_tokens == 0 {
            continue;
        }
        let model =
            non_empty_json_string(message.get("model")).map(|model| format!("[pi] {model}"));
        let display_cost = usage_value
            .get("cost")
            .and_then(|cost| cost.get("total"))
            .and_then(Value::as_f64);
        let cost = calculate_cost_for_usage(model.as_deref(), usage, display_cost, mode, pricing);
        let missing_pricing_model =
            missing_pricing_model_for_usage(model.as_deref(), usage, display_cost, mode, pricing);
        let data = UsageEntry {
            session_id: Some(session_id.clone()),
            timestamp: timestamp_text,
            version: None,
            message: UsageMessage {
                usage,
                model: model.clone(),
                id: None,
            },
            cost_usd: display_cost,
            request_id: None,
            is_api_error_message: None,
            is_sidechain: None,
        };
        entries.push(LoadedEntry {
            date: format_date_tz(timestamp, tz),
            timestamp,
            project: Arc::from(project.as_str()),
            session_id: Arc::from(session_id.as_str()),
            project_path: Arc::from(project.as_str()),
            cost,
            extra_total_tokens,
            credits: None,
            message_count: None,
            model,
            data,
            usage_limit_reset_time: None,
            missing_pricing_model,
        });
    }
    Ok(entries)
}

fn is_pi_message_usage(value: &Value) -> bool {
    let message_type = value.get("type").and_then(Value::as_str);
    if message_type.is_some_and(|message_type| message_type != "message") {
        return false;
    }
    let Some(message) = value.get("message") else {
        return false;
    };
    message.get("role").and_then(Value::as_str) == Some("assistant")
        && message.get("usage").is_some()
}

fn extract_session_id(path: &Path) -> String {
    let filename = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown");
    filename
        .split_once('_')
        .map_or(filename, |(_, session)| session)
        .to_string()
}

fn extract_project(path: &Path) -> String {
    let mut previous_was_sessions = false;
    for component in path.components() {
        let segment = component.as_os_str().to_string_lossy();
        if previous_was_sessions {
            return segment.into_owned();
        }
        previous_was_sessions = segment == "sessions";
    }
    "unknown".to_string()
}

pub(super) fn entry_id(entry: &LoadedEntry) -> String {
    [
        "pi",
        entry.project.as_ref(),
        entry.session_id.as_ref(),
        entry.data.timestamp.as_str(),
        entry.model.as_deref().unwrap_or_default(),
        &entry.data.message.usage.input_tokens.to_string(),
        &entry.data.message.usage.output_tokens.to_string(),
        &entry
            .data
            .message
            .usage
            .cache_creation_input_tokens
            .to_string(),
        &entry.data.message.usage.cache_read_input_tokens.to_string(),
        &entry.extra_total_tokens.to_string(),
        &entry.cost.to_string(),
    ]
    .join(":")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::fs_fixture;

    #[test]
    fn falls_back_to_total_tokens_when_pi_parts_are_missing() {
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"totalTokens":333}}}"#,
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");

        let entries = read_session_file(&file, None, CostMode::Display, None).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.output_tokens, 333);
        assert_eq!(entries[0].extra_total_tokens, 0);
    }

    #[test]
    fn sets_missing_pricing_model_when_model_not_in_pricing() {
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"unknown-model-xyz","usage":{"input":100,"output":200}}}"#,
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");

        // Use Calculate mode with an empty PricingMap so model won't be found
        let pricing = PricingMap::default();
        let entries = read_session_file(&file, None, CostMode::Calculate, Some(&pricing)).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].missing_pricing_model.as_deref(),
            Some("[pi] unknown-model-xyz")
        );
    }

    #[test]
    fn no_missing_pricing_model_in_display_mode() {
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"unknown-model-xyz","usage":{"input":100,"output":200}}}"#,
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");

        let pricing = PricingMap::default();
        let entries = read_session_file(&file, None, CostMode::Display, Some(&pricing)).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].missing_pricing_model, None);
    }

    #[test]
    fn no_missing_pricing_model_when_auto_mode_has_display_cost() {
        let fixture = fs_fixture!({
            "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"unknown-model-xyz","usage":{"input":100,"output":200,"cost":{"total":0.05}}}}"#,
        });
        let file = fixture.path("sessions/project-a/agent_session-a.jsonl");

        let pricing = PricingMap::default();
        let entries = read_session_file(&file, None, CostMode::Auto, Some(&pricing)).unwrap();

        assert_eq!(entries.len(), 1);
        // In Auto mode with a display cost present, no missing pricing warning
        assert_eq!(entries[0].missing_pricing_model, None);
    }
}
