use std::{fs, path::Path, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;
use serde_json::Value;

use super::paths;
use crate::{
    LoadedEntry, PricingMap, Result, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    calculate_cost_for_usage, cli::CostMode, format_date_tz,
    missing_pricing_model_for_usage, parse_ts_timestamp, parse_tz,
};

/// Represents the token usage data from a Mistral Vibe session
#[derive(Debug, Clone)]
pub(super) struct VibeSessionUsage {
    pub session_id: String,
    pub start_time: String,
    pub end_time: Option<String>,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_llm_tokens: Option<u64>,
    pub cost: Option<f64>,
    pub input_price_per_million: Option<f64>,
    pub output_price_per_million: Option<f64>,
    pub model: Option<String>,
    pub session_path: std::path::PathBuf,
}

/// Parse a meta.json file and extract usage data
pub(super) fn parse_meta_json(path: &Path) -> Result<Option<VibeSessionUsage>> {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };

    let value: Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };

    let record = match value.as_object() {
        Some(r) => r,
        None => return Ok(None),
    };

    // Extract session metadata
    let session_id = match paths::extract_session_id_from_path(path) {
        Some(id) => id,
        None => {
            // Try to get from meta.json
            match record.get("session_id").and_then(Value::as_str) {
                Some(id) => id.to_string(),
                None => return Ok(None),
            }
        }
    };

    let start_time = match record.get("start_time").and_then(Value::as_str) {
        Some(t) => t.to_string(),
        None => return Ok(None),
    };

    let end_time = record.get("end_time").and_then(Value::as_str).map(|t| t.to_string());

    // Extract token usage from stats
    let stats = match record.get("stats").and_then(Value::as_object) {
        Some(s) => s,
        None => return Ok(None),
    };

    let prompt_tokens = match stats.get("session_prompt_tokens").and_then(Value::as_u64) {
        Some(t) => t,
        None => return Ok(None),
    };

    let completion_tokens = match stats.get("session_completion_tokens").and_then(Value::as_u64) {
        Some(t) => t,
        None => return Ok(None),
    };

    let total_llm_tokens = stats.get("session_total_llm_tokens").and_then(Value::as_u64);

    let cost = stats.get("session_cost").and_then(Value::as_f64);

    let input_price_per_million = stats.get("input_price_per_million").and_then(Value::as_f64);
    let output_price_per_million = stats.get("output_price_per_million").and_then(Value::as_f64);

    // Try to get model from meta.json
    let model = record.get("model").and_then(Value::as_str).map(|m| m.to_string());

    Ok(Some(VibeSessionUsage {
        session_id,
        start_time,
        end_time,
        prompt_tokens,
        completion_tokens,
        total_llm_tokens,
        cost,
        input_price_per_million,
        output_price_per_million,
        model,
        session_path: path.to_path_buf(),
    }))
}

/// Convert a VibeSessionUsage to a LoadedEntry
pub(super) fn session_to_loaded_entry(
    session: VibeSessionUsage,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> Option<LoadedEntry> {
    // Parse the start timestamp
    let timestamp = match parse_ts_timestamp(&session.start_time) {
        Some(ts) => ts,
        None => return None,
    };

    let date = format_date_tz(timestamp, tz);

    // Build usage data
    let usage = TokenUsageRaw {
        input_tokens: session.prompt_tokens,
        output_tokens: session.completion_tokens,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
        speed: None,
        cache_creation: None,
    };

    // Calculate cost
    let cost = if mode == CostMode::Display {
        0.0
    } else {
        let model = session.model.as_deref();
        calculate_cost_for_usage(
            model,
            usage,
            None,
            CostMode::Calculate,
            pricing,
        )
    };

    // Check for missing pricing
    let missing_pricing_model = if mode == CostMode::Display {
        None
    } else {
        missing_pricing_model_for_usage(
            session.model.as_deref(),
            usage,
            session.cost,
            mode,
            pricing,
        )
    };

    // Build the usage entry
    let data = UsageEntry {
        session_id: Some(session.session_id.clone()),
        timestamp: session.start_time.clone(),
        version: None,
        message: UsageMessage {
            usage,
            model: session.model.clone(),
            id: None,
        },
        cost_usd: session.cost,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };

    Some(LoadedEntry {
        data,
        timestamp,
        date,
        project: Arc::from("vibe"),
        session_id: Arc::from(session.session_id),
        project_path: Arc::from("Mistral Vibe"),
        cost,
        extra_total_tokens: session
            .total_llm_tokens
            .map(|t| t.saturating_sub(session.prompt_tokens + session.completion_tokens))
            .unwrap_or(0),
        credits: None,
        message_count: None,
        model: session.model,
        usage_limit_reset_time: None,
        missing_pricing_model,
    })
}

/// Load all entries from Mistral Vibe session directories
pub(super) fn load_entries(
    shared: &crate::cli::SharedArgs,
    pricing: Option<&PricingMap>,
) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let mode = shared.mode;
    let mut entries = Vec::new();

    for session_dir in paths::discover_session_dirs()? {
        let meta_path = session_dir.join("meta.json");
        if !meta_path.is_file() {
            continue;
        }

        let session = match parse_meta_json(&meta_path)? {
            Some(s) => s,
            None => continue,
        };

        if let Some(entry) = session_to_loaded_entry(session, tz.as_ref(), mode, pricing) {
            entries.push(entry);
        }
    }

    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::fs_fixture;
    use crate::cli::{CostMode, SharedArgs};

    #[test]
    fn parses_meta_json_with_all_fields() {
        let fixture = fs_fixture!({
            "session_20260615_172447_abc123/meta.json": serde_json::json!({
                "session_id": "abc123",
                "start_time": "2026-06-15T17:24:47.685591+00:00",
                "end_time": "2026-06-15T17:30:02.272580+00:00",
                "model": "mistral-large",
                "stats": {
                    "session_prompt_tokens": 1000,
                    "session_completion_tokens": 500,
                    "session_total_llm_tokens": 1500,
                    "session_cost": 0.0015,
                    "input_price_per_million": 1.5,
                    "output_price_per_million": 7.5
                }
            })
        });

        let meta_path = fixture.path("session_20260615_172447_abc123/meta.json");
        let session = parse_meta_json(meta_path).unwrap().unwrap();

        assert_eq!(session.session_id, "abc123");
        assert_eq!(session.prompt_tokens, 1000);
        assert_eq!(session.completion_tokens, 500);
        assert_eq!(session.total_llm_tokens, Some(1500));
        assert_eq!(session.cost, Some(0.0015));
        assert_eq!(session.model, Some("mistral-large".to_string()));
    }

    #[test]
    fn parses_meta_json_with_minimal_fields() {
        let fixture = fs_fixture!({
            "session_20260615_172447_abc123/meta.json": serde_json::json!({
                "start_time": "2026-06-15T17:24:47Z",
                "stats": {
                    "session_prompt_tokens": 100,
                    "session_completion_tokens": 50
                }
            })
        });

        let meta_path = fixture.path("session_20260615_172447_abc123/meta.json");
        let session = parse_meta_json(meta_path).unwrap().unwrap();

        assert_eq!(session.prompt_tokens, 100);
        assert_eq!(session.completion_tokens, 50);
        assert_eq!(session.total_llm_tokens, None);
        assert_eq!(session.cost, None);
        assert_eq!(session.model, None);
    }

    #[test]
    fn returns_none_for_invalid_json() {
        let fixture = fs_fixture!({
            "session_20260615_172447_abc123/meta.json": "not valid json",
        });

        let meta_path = fixture.path("session_20260615_172447_abc123/meta.json");
        let session = parse_meta_json(meta_path).unwrap();
        assert!(session.is_none());
    }

    #[test]
    fn returns_none_for_missing_stats() {
        let fixture = fs_fixture!({
            "session_20260615_172447_abc123/meta.json": serde_json::json!({
                "start_time": "2026-06-15T17:24:47Z"
            })
        });

        let meta_path = fixture.path("session_20260615_172447_abc123/meta.json");
        let session = parse_meta_json(meta_path).unwrap();
        assert!(session.is_none());
    }

    #[test]
    fn converts_session_to_loaded_entry_with_display_mode() {
        let fixture = fs_fixture!({
            "session_20260615_172447_abc123/meta.json": serde_json::json!({
                "session_id": "abc123",
                "start_time": "2026-06-15T17:24:47Z",
                "model": "mistral-large",
                "stats": {
                    "session_prompt_tokens": 1000,
                    "session_completion_tokens": 500,
                    "session_total_llm_tokens": 1500
                }
            })
        });

        let meta_path = fixture.path("session_20260615_172447_abc123/meta.json");
        let session = parse_meta_json(meta_path).unwrap().unwrap();
        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        let entry = session_to_loaded_entry(session, parse_tz(Some("UTC")), shared.mode, None).unwrap();

        assert_eq!(entry.session_id.as_ref(), "abc123");
        assert_eq!(entry.date, "2026-06-15");
        assert_eq!(entry.data.message.usage.input_tokens, 1000);
        assert_eq!(entry.data.message.usage.output_tokens, 500);
        assert_eq!(entry.model.as_deref(), Some("mistral-large"));
        assert_eq!(entry.project_path.as_ref(), "Mistral Vibe");
    }

    #[test]
    fn calculates_extra_total_tokens() {
        let fixture = fs_fixture!({
            "session_20260615_172447_abc123/meta.json": serde_json::json!({
                "session_id": "abc123",
                "start_time": "2026-06-15T17:24:47Z",
                "model": "mistral-large",
                "stats": {
                    "session_prompt_tokens": 1000,
                    "session_completion_tokens": 500,
                    "session_total_llm_tokens": 2000
                }
            })
        });

        let meta_path = fixture.path("session_20260615_172447_abc123/meta.json");
        let session = parse_meta_json(meta_path).unwrap().unwrap();
        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        let entry = session_to_loaded_entry(session, parse_tz(Some("UTC")), shared.mode, None).unwrap();

        // extra_total_tokens = total_llm_tokens - (prompt + completion) = 2000 - 1500 = 500
        assert_eq!(entry.extra_total_tokens, 500);
    }

    #[test]
    fn handles_zero_extra_tokens() {
        let fixture = fs_fixture!({
            "session_20260615_172447_abc123/meta.json": serde_json::json!({
                "session_id": "abc123",
                "start_time": "2026-06-15T17:24:47Z",
                "stats": {
                    "session_prompt_tokens": 1000,
                    "session_completion_tokens": 500,
                    "session_total_llm_tokens": 1500
                }
            })
        });

        let meta_path = fixture.path("session_20260615_172447_abc123/meta.json");
        let session = parse_meta_json(meta_path).unwrap().unwrap();
        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        let entry = session_to_loaded_entry(session, parse_tz(Some("UTC")), shared.mode, None).unwrap();

        // extra_total_tokens = 1500 - (1000 + 500) = 0
        assert_eq!(entry.extra_total_tokens, 0);
    }
}
