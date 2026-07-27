use std::{
    collections::HashSet,
    fs,
    path::Path,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;

use ccusage_adapter_common::jsonl;

use super::paths;
use crate::{
    LoadedEntry, PricingMap, Result, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    apply_total_token_fallback, calculate_cost_for_usage,
    cli::{CostMode, SharedArgs},
    debug_log,
    fast::LinePrefilter,
    format_date_tz, format_rfc3339_millis, missing_pricing_model_for_candidates,
    parse_ts_timestamp, parse_tz, read_files_parallel,
};

const DEFAULT_QWEN_MODEL: &str = "unknown";

/// A single parsed Qwen chat record. Only the fields ccusage consumes are
/// declared; serde skips everything else.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QwenLine {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    r#type: Option<String>,
    usage_metadata: Option<QwenUsageMetadata>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    timestamp: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    session_id: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    model: Option<String>,
}

/// Gemini-style usage metadata block carried by Qwen assistant records.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QwenUsageMetadata {
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    prompt_token_count: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    candidates_token_count: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    thoughts_token_count: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cached_content_token_count: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    total_token_count: u64,
}

pub(super) fn load_entries(shared: &SharedArgs) -> Result<Vec<LoadedEntry>> {
    let pricing = if shared.mode == CostMode::Display {
        None
    } else {
        Some(PricingMap::load_with_overrides(
            shared.offline,
            crate::log_level() != Some(0),
            shared.pricing_overrides.iter(),
        ))
    };
    let tz = parse_tz(shared.timezone.as_deref());
    let files = paths::discover_chat_files()?;
    // Read chat files in parallel; the first-wins dedup runs sequentially over
    // the original discovery order so the surviving record per id matches the
    // single-threaded read.
    let loaded = read_files_parallel(&files, shared.single_thread, |file| {
        read_chat_file(file, tz.as_ref(), shared.mode, pricing.as_ref(), shared).unwrap_or_else(
            |error| {
                debug_log(
                    shared,
                    format!("Failed to read Qwen chat file {}: {error}", file.display()),
                );
                Vec::new()
            },
        )
    });
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for file_entries in loaded {
        for entry in file_entries {
            if seen.insert(entry_id(&entry)) {
                entries.push(entry);
            }
        }
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

fn read_chat_file(
    file: &Path,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
    shared: &SharedArgs,
) -> Result<Vec<LoadedEntry>> {
    let fallback = file_timestamp(file, shared);
    let content = fs::read(file)?;
    // Every usable Qwen line carries token counts under the `usageMetadata`
    // key, so lines without it are skipped before JSON parsing.
    let prefilter = LinePrefilter::all(&[br#""usageMetadata""#]);
    let mut entries = Vec::new();
    for record in jsonl::records::<QwenLine>(&content, Some(&prefilter)) {
        if let Some(entry) = parse_line(file, fallback, &record, tz, mode, pricing) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

fn parse_line(
    file: &Path,
    fallback: TimestampMs,
    record: &QwenLine,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> Option<LoadedEntry> {
    if record.r#type.as_deref() != Some("assistant") {
        return None;
    }
    let usage = record.usage_metadata.as_ref()?;
    let input_tokens = usage.prompt_token_count;
    let output_tokens = usage.candidates_token_count;
    let reasoning_tokens = usage.thoughts_token_count;
    let cache_read_tokens = usage.cached_content_token_count;
    let total_tokens = usage.total_token_count;
    let display_usage = TokenUsageRaw {
        input_tokens,
        output_tokens,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: cache_read_tokens,
        speed: None,
        cache_creation: None,
    };
    let (display_usage, extra_total_tokens) =
        apply_total_token_fallback(display_usage, reasoning_tokens, total_tokens);
    if display_usage.input_tokens == 0
        && display_usage.output_tokens == 0
        && display_usage.cache_read_input_tokens == 0
        && extra_total_tokens == 0
    {
        return None;
    }

    let timestamp_text = record
        .timestamp
        .clone()
        .and_then(|value| parse_ts_timestamp(&value).map(|_| value))
        .unwrap_or_else(|| format_rfc3339_millis(fallback));
    let timestamp = parse_ts_timestamp(&timestamp_text).unwrap_or(fallback);
    let project = paths::project_from_file(file).unwrap_or_else(|| "unknown".to_string());
    let session_id = record.session_id.clone().unwrap_or_else(|| {
        let stem = file
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("unknown");
        format!("{project}-{stem}")
    });
    let model = record
        .model
        .clone()
        .unwrap_or_else(|| DEFAULT_QWEN_MODEL.to_string());
    let billable_usage = TokenUsageRaw {
        output_tokens: display_usage
            .output_tokens
            .saturating_add(extra_total_tokens),
        cache_creation: None,
        ..display_usage
    };
    let cost = calculate_qwen_cost(&model, billable_usage, mode, pricing);
    let missing_pricing_model = missing_qwen_pricing(&model, billable_usage, mode, pricing);
    let data = UsageEntry {
        session_id: Some(session_id.clone()),
        timestamp: timestamp_text,
        version: None,
        message: UsageMessage {
            usage: display_usage,
            model: Some(model.clone()),
            id: None,
        },
        cost_usd: None,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };
    Some(LoadedEntry {
        data,
        timestamp,
        date: format_date_tz(timestamp, tz),
        project: Arc::from("qwen"),
        session_id: Arc::from(session_id),
        project_path: Arc::from(project),
        cost,
        credits: None,
        model: Some(model),
        message_count: None,
        usage_limit_reset_time: None,
        missing_pricing_model,
        extra_total_tokens,
    })
}

fn calculate_qwen_cost(
    model: &str,
    usage: TokenUsageRaw,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> f64 {
    for candidate in qwen_model_candidates(model) {
        if mode == CostMode::Display
            || pricing.is_some_and(|pricing| pricing.find(&candidate).is_some())
        {
            return calculate_cost_for_usage(Some(&candidate), usage, None, mode, pricing);
        }
    }
    0.0
}

fn missing_qwen_pricing(
    model: &str,
    usage: TokenUsageRaw,
    mode: CostMode,
    pricing: Option<&PricingMap>,
) -> Option<String> {
    if mode == CostMode::Display {
        return None;
    }
    missing_pricing_model_for_candidates(
        model,
        qwen_model_candidates(model),
        crate::total_usage_tokens(usage),
        pricing,
    )
}

fn qwen_model_candidates(model: &str) -> Vec<String> {
    vec![
        model.to_string(),
        format!("qwen/{model}"),
        format!("alibaba/{model}"),
    ]
}

fn file_timestamp(file: &Path, shared: &SharedArgs) -> TimestampMs {
    match fs::metadata(file)
        .and_then(|metadata| metadata.modified())
        .and_then(|modified| {
            modified
                .duration_since(UNIX_EPOCH)
                .map_err(std::io::Error::other)
        }) {
        Ok(duration) => TimestampMs::from_millis(duration.as_millis().min(i64::MAX as u128) as i64),
        Err(error) => {
            crate::debug_log(
                shared,
                format!(
                    "Failed to read Qwen chat file timestamp for {}: {error}",
                    file.display()
                ),
            );
            system_time_timestamp(SystemTime::now())
        }
    }
}

fn system_time_timestamp(time: SystemTime) -> TimestampMs {
    time.duration_since(UNIX_EPOCH).map_or_else(
        |_| TimestampMs::UNIX_EPOCH,
        |duration| TimestampMs::from_millis(duration.as_millis().min(i64::MAX as u128) as i64),
    )
}

fn entry_id(entry: &LoadedEntry) -> String {
    let usage = entry.data.message.usage;
    serde_json::json!([
        entry.session_id.as_ref(),
        entry.data.timestamp.as_str(),
        entry.model.as_deref().unwrap_or_default(),
        usage.input_tokens,
        usage.output_tokens,
        usage.cache_read_input_tokens,
        entry.extra_total_tokens
    ])
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculate_qwen_cost_returns_explicit_zero_price() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "free-model": {
                    "input_cost_per_token": 0,
                    "output_cost_per_token": 0,
                    "cache_creation_input_token_cost": 0,
                    "cache_read_input_token_cost": 0
                },
                "qwen/free-model": {
                    "input_cost_per_token": 1,
                    "output_cost_per_token": 1
                }
            }"#,
        );

        let cost = calculate_qwen_cost(
            "free-model",
            TokenUsageRaw {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                speed: None,
                cache_creation: None,
            },
            CostMode::Calculate,
            Some(&pricing),
        );

        assert_eq!(cost, 0.0);
    }

    #[test]
    fn falls_back_to_total_token_count_when_qwen_parts_are_missing() {
        let record = serde_json::from_value::<QwenLine>(serde_json::json!({
            "type": "assistant",
            "timestamp": "2026-01-02T00:00:00.000Z",
            "sessionId": "session-a",
            "model": "qwen3-coder",
            "usageMetadata": {
                "totalTokenCount": 321
            }
        }))
        .unwrap();
        let entry = parse_line(
            Path::new("/tmp/project/chat.jsonl"),
            TimestampMs::UNIX_EPOCH,
            &record,
            None,
            CostMode::Auto,
            None,
        )
        .unwrap();

        assert_eq!(entry.data.message.usage.output_tokens, 321);
        assert_eq!(entry.extra_total_tokens, 0);
    }

    #[test]
    fn entry_id_is_unambiguous_for_colon_fields() {
        let entry = LoadedEntry {
            data: UsageEntry {
                session_id: Some("session:1".to_string()),
                timestamp: "2026-01-02T00:00:00.000Z".to_string(),
                version: None,
                message: UsageMessage {
                    usage: TokenUsageRaw {
                        input_tokens: 1,
                        output_tokens: 2,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: 3,
                        speed: None,
                        cache_creation: None,
                    },
                    model: Some("model:1".to_string()),
                    id: None,
                },
                cost_usd: None,
                request_id: None,
                is_api_error_message: None,
                is_sidechain: None,
            },
            timestamp: TimestampMs::UNIX_EPOCH,
            date: "2026-01-02".to_string(),
            project: Arc::from("qwen"),
            session_id: Arc::from("session:1"),
            project_path: Arc::from("project"),
            cost: 0.0,
            extra_total_tokens: 4,
            credits: None,
            message_count: None,
            model: Some("model:1".to_string()),
            usage_limit_reset_time: None,
            missing_pricing_model: None,
        };

        assert_eq!(
            entry_id(&entry),
            r#"["session:1","2026-01-02T00:00:00.000Z","model:1",1,2,3,4]"#
        );
    }
}
