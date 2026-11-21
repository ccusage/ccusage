use ccusage_core::cost::PriceTable;
use ccusage_core::{
    cost::{EntryPricing, UsageGranularity, price_entry},
    pricing::MatchKind,
};
use std::{path::Path, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;
use serde_json::Value;

use ccusage_adapter_common::{jsonl, units::UnitError};

use super::paths;
use crate::{
    Diagnostics, LoadedEntry, TimestampMs, TokenUsageRaw, date_at, fast::LinePrefilter,
    parse_ts_timestamp,
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
    #[serde(default, deserialize_with = "present_u64")]
    candidates_token_count: Option<u64>,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    thoughts_token_count: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cached_content_token_count: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    total_token_count: u64,
}

pub(super) struct QwenEntry {
    pub(super) entry: LoadedEntry,
    source_timestamp: String,
}

impl std::ops::Deref for QwenEntry {
    type Target = LoadedEntry;

    fn deref(&self) -> &Self::Target {
        &self.entry
    }
}

fn present_u64<'de, D>(deserializer: D) -> std::result::Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    jsonl::lenient_u64(deserializer).map(Some)
}

pub(super) fn read_chat_file(
    file: &Path,
    tz: &JiffTimeZone,
    pricing: PriceTable<'_>,
    diagnostics: &Diagnostics,
) -> std::result::Result<Vec<QwenEntry>, UnitError> {
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
) -> std::result::Result<Option<QwenEntry>, MissingTimestamp> {
    if record.r#type.as_deref() != Some("assistant") {
        return Ok(None);
    }
    let Some(usage) = record.usage_metadata.as_ref() else {
        return Ok(None);
    };
    // NOTE(2026-11-20, #2363): Qwen Code counts `cachedContentTokenCount` inside `promptTokenCount` (QwenLM/qwen-code `packages/cli/src/commands/review/cost-ledger.ts`, `packages/core/src/core/anthropicContentGenerator/usage.ts`). It takes `totalTokenCount - promptTokenCount` as output including thinking, because its OpenAI-compatible converter sets `candidatesTokenCount` from `completion_tokens` and `thoughtsTokenCount` from its `reasoning_tokens` subset (`packages/core/src/core/openaiContentGenerator/converter.ts`), while Gemini puts thinking outside the candidates. Without a total, thinking is taken to be inside candidates, as Qwen Code does.
    let prompt_tokens = usage.prompt_token_count;
    let total_tokens = usage.total_token_count;
    let cache_read_tokens = usage.cached_content_token_count.min(prompt_tokens);
    let candidates_tokens = usage.candidates_token_count.unwrap_or(0);
    let output_tokens = if total_tokens > prompt_tokens {
        candidates_tokens.min(total_tokens - prompt_tokens)
    } else {
        candidates_tokens
    };
    let raw_usage = TokenUsageRaw {
        input_tokens: prompt_tokens - cache_read_tokens,
        output_tokens,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: cache_read_tokens,
        speed: None,
        cache_creation: None,
    };
    let remainder = ccusage_core::AdditionalTokens::from_total(raw_usage, 0, total_tokens);
    let reasoning = usage.thoughts_token_count.min(remainder.residual);
    let additional = ccusage_core::AdditionalTokens {
        reasoning,
        residual: remainder.residual - reasoning,
    };
    let (display_usage, extra_total_tokens) = if usage.candidates_token_count.is_some() {
        (raw_usage, remainder.residual)
    } else {
        remainder.for_report(raw_usage)
    };
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
    let (cost, missing_pricing_model, estimated_pricing) =
        price_qwen(&model, raw_usage, additional, timestamp, pricing);
    Ok(Some(QwenEntry {
        source_timestamp: timestamp_text.clone(),
        entry: LoadedEntry {
            usage: display_usage,
            message_id: None,
            version: None,
            recorded_cost: None,
            timestamp,
            date: date_at(timestamp, tz),
            project: Arc::from("qwen"),
            session_id: Arc::from(session_id),
            project_path: Arc::from(project),
            cost,
            credits: None,
            model: Some(model),
            message_count: None,
            usage_limit_reset_time: None,
            missing_pricing_model,
            estimated_pricing,
            extra_total_tokens,
            stored_cost: None,
        },
    }))
}

fn price_qwen(
    model: &str,
    usage: TokenUsageRaw,
    additional: ccusage_core::AdditionalTokens,
    timestamp: TimestampMs,
    pricing: PriceTable<'_>,
) -> (f64, Option<String>, bool) {
    let candidates = qwen_model_candidates(model);
    let candidates: Vec<_> = candidates
        .iter()
        .map(|name| (name.as_str(), MatchKind::Fuzzy))
        .collect();
    let priced = price_entry(
        EntryPricing {
            granularity: UsageGranularity::Request,
            usage,
            reasoning_tokens: additional.reasoning,
            residual_tokens: additional.residual,
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
        priced.estimated,
    )
}

fn qwen_model_candidates(model: &str) -> Vec<String> {
    vec![
        model.to_string(),
        format!("qwen/{model}"),
        format!("alibaba/{model}"),
    ]
}

pub(super) fn entry_id(entry: &QwenEntry) -> String {
    let usage = entry.usage;
    serde_json::json!([
        entry.session_id.as_ref(),
        entry.source_timestamp.as_str(),
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

        let (cost, _, _) = price_qwen(
            "free-model",
            TokenUsageRaw {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                speed: None,
                cache_creation: None,
            },
            ccusage_core::AdditionalTokens::default(),
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
            &jiff::tz::TimeZone::UTC,
            PriceTable::for_map(
                ccusage_core::cli::CostMode::Auto,
                &ccusage_core::PricingMap::default(),
            ),
        )
        .unwrap()
        .unwrap();

        assert_eq!(entry.usage.output_tokens, 321);
        assert_eq!(entry.extra_total_tokens, 0);
    }

    fn parse_usage(usage: serde_json::Value) -> QwenEntry {
        let record = serde_json::from_value::<QwenLine>(serde_json::json!({
            "type": "assistant",
            "timestamp": "2026-02-23T14:24:56.857Z",
            "sessionId": "s",
            "model": "qwen3-coder-plus",
            "usageMetadata": usage
        }))
        .unwrap();
        parse_line(
            Path::new("/tmp/project/chat.jsonl"),
            &record,
            &jiff::tz::TimeZone::UTC,
            PriceTable::Display,
        )
        .unwrap()
        .unwrap()
    }

    fn reported_total(entry: &QwenEntry) -> u64 {
        ccusage_core::total_usage_tokens(entry.usage) + entry.extra_total_tokens
    }

    #[test]
    fn cached_tokens_are_part_of_the_prompt_count() {
        let entry = parse_usage(serde_json::json!({
            "promptTokenCount": 1000,
            "candidatesTokenCount": 200,
            "cachedContentTokenCount": 400,
            "totalTokenCount": 1200
        }));

        assert_eq!(entry.usage.input_tokens, 600);
        assert_eq!(entry.usage.cache_read_input_tokens, 400);
        assert_eq!(entry.usage.output_tokens, 200);
        assert_eq!(reported_total(&entry), 1200);
    }

    #[test]
    fn thoughts_inside_candidates_are_not_added_again() {
        let entry = parse_usage(serde_json::json!({
            "promptTokenCount": 1000,
            "candidatesTokenCount": 200,
            "thoughtsTokenCount": 50,
            "cachedContentTokenCount": 400,
            "totalTokenCount": 1200
        }));

        assert_eq!(entry.usage.output_tokens, 200);
        assert_eq!(entry.extra_total_tokens, 0);
        assert_eq!(reported_total(&entry), 1200);
    }

    #[test]
    fn thoughts_outside_candidates_count_once_as_reasoning() {
        let entry = parse_usage(serde_json::json!({
            "promptTokenCount": 1000,
            "candidatesTokenCount": 200,
            "thoughtsTokenCount": 50,
            "cachedContentTokenCount": 400,
            "totalTokenCount": 1250
        }));

        assert_eq!(entry.usage.output_tokens, 200);
        assert_eq!(entry.extra_total_tokens, 50);
        assert_eq!(reported_total(&entry), 1250);
    }

    #[test]
    fn cached_prompt_tokens_are_billed_once_at_the_cache_read_rate() {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(
            r#"{
                "qwen3-coder-plus": {
                    "litellm_provider": "dashscope",
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000005,
                    "cache_read_input_token_cost": 0.0000001
                }
            }"#,
        );
        let record = serde_json::from_value::<QwenLine>(serde_json::json!({
            "type": "assistant",
            "timestamp": "2026-02-23T14:24:56.857Z",
            "sessionId": "s",
            "model": "qwen3-coder-plus",
            "usageMetadata": {
                "promptTokenCount": 1000,
                "candidatesTokenCount": 200,
                "thoughtsTokenCount": 50,
                "cachedContentTokenCount": 400,
                "totalTokenCount": 1250
            }
        }))
        .unwrap();
        let entry = parse_line(
            Path::new("/tmp/project/chat.jsonl"),
            &record,
            &jiff::tz::TimeZone::UTC,
            PriceTable::Calculate(&pricing),
        )
        .unwrap()
        .unwrap();

        let expected = 600.0 * 0.000001 + 400.0 * 0.0000001 + 250.0 * 0.000005;
        assert!((entry.cost - expected).abs() < 1e-12, "{}", entry.cost);
    }

    #[test]
    fn thinking_with_zero_candidates_stays_reasoning() {
        let entry = parse_usage(serde_json::json!({
            "promptTokenCount": 1000,
            "candidatesTokenCount": 0,
            "thoughtsTokenCount": 50,
            "totalTokenCount": 1050
        }));

        assert_eq!(entry.usage.output_tokens, 0);
        assert_eq!(entry.extra_total_tokens, 50);
    }

    #[test]
    fn without_a_total_thoughts_are_taken_to_be_inside_candidates() {
        let entry = parse_usage(serde_json::json!({
            "promptTokenCount": 500,
            "candidatesTokenCount": 120,
            "thoughtsTokenCount": 30,
            "cachedContentTokenCount": 80
        }));

        assert_eq!(entry.usage.input_tokens, 420);
        assert_eq!(entry.usage.cache_read_input_tokens, 80);
        assert_eq!(entry.usage.output_tokens, 120);
        assert_eq!(entry.extra_total_tokens, 0);
    }

    #[test]
    fn entry_id_is_unambiguous_for_colon_fields() {
        let entry = QwenEntry {
            source_timestamp: "2026-01-02T00:00:00.000Z".to_string(),
            entry: LoadedEntry {
                usage: TokenUsageRaw {
                    input_tokens: 1,
                    output_tokens: 2,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 3,
                    speed: None,
                    cache_creation: None,
                },
                message_id: None,
                version: None,
                recorded_cost: None,
                timestamp: TimestampMs::UNIX_EPOCH,
                date: ccusage_core::cli::IsoDate::parse("2026-01-02").unwrap(),
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
                estimated_pricing: false,
            },
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
