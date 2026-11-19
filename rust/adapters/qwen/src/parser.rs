use ccusage_core::cost::PriceTable;
use ccusage_core::{
    cost::{EntryPricing, price_entry},
    pricing::MatchKind,
};
use std::{path::Path, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;
use serde_json::Value;

use ccusage_adapter_common::{jsonl, units::UnitError};

use super::paths;
use crate::{
    Diagnostics, LoadedEntry, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    apply_total_token_fallback, fast::LinePrefilter, format_date_tz, parse_ts_timestamp,
};

const DEFAULT_QWEN_MODEL: &str = "unknown";

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

pub(super) fn read_chat_file(
    file: &Path,
    tz: &JiffTimeZone,
    pricing: PriceTable<'_>,
    diagnostics: &Diagnostics,
) -> std::result::Result<Vec<LoadedEntry>, UnitError> {
    let prefilter = LinePrefilter::all(&[br#""usageMetadata""#]);
    let mut entries = Vec::new();
    jsonl::read_records::<QwenLine>(
        file,
        Some(&prefilter),
        diagnostics,
        is_qwen_usage,
        |number, record, diagnostics| match parse_line(file, &record, tz, pricing) {
            Ok(Some(entry)) => entries.push(entry),
            Ok(None) => {}
            Err(MissingTimestamp) => diagnostics.record(ccusage_core::Skip::record(
                file,
                number,
                ccusage_core::SkipKind::InvalidTimestamp,
                "Qwen usage record has no valid timestamp",
            )),
        },
    )?;
    Ok(entries)
}

fn is_qwen_usage(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("assistant")
        && value.get("usageMetadata").is_some()
}

#[derive(Debug)]
struct MissingTimestamp;

fn parse_line(
    file: &Path,
    record: &QwenLine,
    tz: &JiffTimeZone,
    pricing: PriceTable<'_>,
) -> std::result::Result<Option<LoadedEntry>, MissingTimestamp> {
    if record.r#type.as_deref() != Some("assistant") {
        return Ok(None);
    }
    let Some(usage) = record.usage_metadata.as_ref() else {
        return Ok(None);
    };
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
        return Ok(None);
    }

    let timestamp_text = record.timestamp.as_ref().ok_or(MissingTimestamp)?;
    let timestamp = parse_ts_timestamp(timestamp_text).ok_or(MissingTimestamp)?;
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
    let (cost, missing_pricing_model) = price_qwen(&model, billable_usage, timestamp, pricing);
    let data = UsageEntry {
        session_id: Some(session_id.clone()),
        timestamp: timestamp_text.clone(),
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
    Ok(Some(LoadedEntry {
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
        stored_cost: None,
    }))
}

fn price_qwen(
    model: &str,
    usage: TokenUsageRaw,
    timestamp: TimestampMs,
    pricing: PriceTable<'_>,
) -> (f64, Option<String>) {
    let candidates = qwen_model_candidates(model);
    let candidates: Vec<_> = candidates
        .iter()
        .map(|name| (name.as_str(), MatchKind::Fuzzy))
        .collect();
    let priced = price_entry(
        EntryPricing {
            usage,
            reasoning_tokens: 0,
            residual_tokens: 0,
            reported_model: Some(model),
            candidates: &candidates,
            stored_cost: None,
            timestamp: Some(timestamp),
        },
        pricing,
    );
    (
        priced.cost,
        priced
            .missing_pricing
            .then(|| ccusage_core::model_aliases::resolve_model_name(model).into_owned()),
    )
}

fn qwen_model_candidates(model: &str) -> Vec<String> {
    vec![
        model.to_string(),
        format!("qwen/{model}"),
        format!("alibaba/{model}"),
    ]
}

pub(super) fn entry_id(entry: &LoadedEntry) -> String {
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
    fn untimed_jsonl_usage_is_diagnosed_without_using_file_mtime() {
        let fixture = ccusage_test_support::fs_fixture!({
            "chat.jsonl": "{\"type\":\"assistant\",\"usageMetadata\":{\"promptTokenCount\":1}}\n{\"type\":\"assistant\",\"timestamp\":\"invalid\",\"usageMetadata\":{\"promptTokenCount\":1}}\n{\"type\":\"assistant\",\"timestamp\":\"1970-01-01T00:00:00Z\",\"usageMetadata\":{\"promptTokenCount\":1}}\n{\"type\":\"assistant\",\"usageMetadata\":{}}\n"
        });
        let diagnostics = Diagnostics::new("qwen");
        let entries = read_chat_file(
            &fixture.path("chat.jsonl"),
            &JiffTimeZone::UTC,
            PriceTable::Display,
            &diagnostics,
        )
        .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].timestamp, TimestampMs::UNIX_EPOCH);
        assert_eq!(
            diagnostics.skipped(ccusage_core::SkipKind::InvalidTimestamp),
            2
        );
    }

    #[test]
    fn calculate_qwen_cost_returns_explicit_zero_price() {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(
            r#"{
                "free-model": {
                    "litellm_provider": "openai", "input_cost_per_token": 0,
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

        let (cost, _) = price_qwen(
            "free-model",
            TokenUsageRaw {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                speed: None,
                cache_creation: None,
            },
            TimestampMs::UNIX_EPOCH,
            PriceTable::Calculate(&pricing),
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
            &record,
            &jiff::tz::TimeZone::system(),
            PriceTable::for_map(
                ccusage_core::cli::CostMode::Auto,
                &ccusage_core::PricingMap::default(),
            ),
        )
        .unwrap()
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
            stored_cost: None,
        };

        assert_eq!(
            entry_id(&entry),
            r#"["session:1","2026-01-02T00:00:00.000Z","model:1",1,2,3,4]"#
        );
    }
}

#[cfg(test)]
mod pricing_identity_tests {
    use super::*;

    #[test]
    fn adapter_pricing_identity() {
        assert_eq!(
            qwen_model_candidates("qwen3-coder-plus"),
            [
                "qwen3-coder-plus",
                "qwen/qwen3-coder-plus",
                "alibaba/qwen3-coder-plus"
            ]
        );
    }
}
