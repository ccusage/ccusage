use ccusage_core::cost::PriceTable;
use ccusage_core::{
    cost::{EntryPricing, UsageGranularity, price_entry_on_route},
    pricing::{BillingRoute, MatchKind},
};
use std::path::Path;

use ccusage_adapter_common::units::{UnitError, read_json};
use ccusage_core::{Diagnostics, Skip, SkipKind};
use serde_json::Value;

use crate::{TokenUsageRaw, format_rfc3339_millis, parse_ts_timestamp};
use ccusage_core::AdditionalTokens;

const DEFAULT_CODEBUFF_MODEL: &str = "codebuff-unknown";

#[derive(Clone, Default)]
struct AssistantUsage {
    model: Option<String>,
    credits: f64,
    input_tokens: u64,
    output_tokens: Option<u64>,
    cache_creation_input_tokens: u64,
    cache_read_input_tokens: u64,
    total_tokens: u64,
}

pub(super) struct CodebuffEntry {
    pub(super) timestamp: crate::TimestampMs,

    pub(super) session_id: String,
    pub(super) model: String,
    provider: String,
    pub(super) credits: f64,
    pub(super) usage: TokenUsageRaw,
    pub(super) extra_total_tokens: u64,
    pub(super) reported_output_tokens: u64,
    pub(super) dedup_key: String,
}

struct CodebuffContext {
    chat_id: String,
    session_id: String,
}

pub(super) fn load_chat_file(
    path: &Path,
    diagnostics: &Diagnostics,
) -> std::result::Result<Vec<CodebuffEntry>, UnitError> {
    let messages = read_json::<Vec<Value>>(path)?;
    let context = derive_context(path);
    let chat_timestamp = parse_codebuff_chat_id_timestamp(&context.chat_id);
    let mut entries = Vec::new();
    for (ordinal, message) in messages.iter().enumerate() {
        let Some(message) = message.as_object() else {
            continue;
        };
        if !is_assistant_message(message) {
            continue;
        }
        let usage = extract_assistant_usage(message);
        if !has_signal(&usage) {
            continue;
        }
        let Some(timestamp) = message_timestamp(message).or(chat_timestamp) else {
            diagnostics.record(Skip::record(
                path,
                Skip::line_number(ordinal),
                SkipKind::InvalidTimestamp,
                "Codebuff usage record has no valid message or chat timestamp",
            ));
            continue;
        };
        let model = usage
            .model
            .clone()
            .unwrap_or_else(|| DEFAULT_CODEBUFF_MODEL.to_string());
        let raw_usage = TokenUsageRaw {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens.unwrap_or_default(),
            cache_creation_input_tokens: usage.cache_creation_input_tokens,
            cache_read_input_tokens: usage.cache_read_input_tokens,
            speed: None,
            cache_creation: None,
        };
        let additional = AdditionalTokens::from_total(raw_usage, 0, usage.total_tokens);
        let (report_usage, extra_total_tokens) = if usage.output_tokens.is_some() {
            (raw_usage, additional.report_remainder)
        } else {
            additional.for_report(raw_usage)
        };
        let dedup_key = dedup_key(
            message,
            &context.session_id,
            timestamp,
            &model,
            report_usage,
            extra_total_tokens,
            ordinal,
        );
        entries.push(CodebuffEntry {
            timestamp,

            session_id: context.session_id.clone(),
            provider: infer_provider(&model).to_string(),
            model,
            credits: usage.credits,
            usage: raw_usage,
            extra_total_tokens,
            reported_output_tokens: report_usage.output_tokens,
            dedup_key,
        });
    }
    Ok(entries)
}

fn derive_context(path: &Path) -> CodebuffContext {
    let chat_id = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("unknown")
        .to_string();
    let chats_dir = path.parent().and_then(Path::parent);
    let project_dir = chats_dir.and_then(Path::parent);
    let project = project_dir
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("unknown")
        .to_string();
    let channel = project_dir
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("manicode")
        .to_string();
    CodebuffContext {
        session_id: format!("{channel}/{project}/{chat_id}"),
        chat_id,
    }
}

fn is_assistant_message(message: &serde_json::Map<String, Value>) -> bool {
    matches!(
        string_field(message, "variant")
            .or_else(|| string_field(message, "role"))
            .as_deref(),
        Some("ai" | "agent" | "assistant")
    )
}

fn extract_assistant_usage(message: &serde_json::Map<String, Value>) -> AssistantUsage {
    let mut usage = AssistantUsage::default();
    if let Some(metadata) = object_field(message, "metadata") {
        usage.model = string_field(metadata, "model");
        merge_fallback(&mut usage, parse_usage_object(metadata.get("usage")));
        merge_fallback(
            &mut usage,
            parse_usage_object(
                metadata
                    .get("codebuff")
                    .and_then(Value::as_object)
                    .and_then(|codebuff| codebuff.get("usage")),
            ),
        );
        if let Some(run_state_usage) = extract_usage_from_run_state(metadata) {
            merge_fallback(&mut usage, run_state_usage);
        }
    }
    let credits = number_field(message, "credits");
    if credits > 0.0 && usage.credits <= 0.0 {
        usage.credits = credits;
    }
    usage
}

fn extract_usage_from_run_state(
    metadata: &serde_json::Map<String, Value>,
) -> Option<AssistantUsage> {
    let history = metadata
        .get("runState")?
        .get("sessionState")?
        .get("mainAgentState")?
        .get("messageHistory")?
        .as_array()?;
    let mut usage = AssistantUsage::default();
    let mut found = false;
    for item in history.iter().rev() {
        let Some(entry) = item.as_object() else {
            continue;
        };
        if string_field(entry, "role").as_deref() != Some("assistant") {
            continue;
        }
        let Some(provider_options) = object_field(entry, "providerOptions") else {
            continue;
        };
        let mut entry_usage = AssistantUsage::default();
        merge_fallback(
            &mut entry_usage,
            parse_usage_object(provider_options.get("usage")),
        );
        if let Some(codebuff) = object_field(provider_options, "codebuff") {
            merge_fallback(&mut entry_usage, parse_usage_object(codebuff.get("usage")));
            entry_usage.model = string_field(codebuff, "model").or(entry_usage.model);
        }
        if has_signal(&entry_usage) || entry_usage.model.is_some() {
            found = true;
        }
        merge_fallback(&mut usage, entry_usage);
    }
    found.then_some(usage)
}

fn parse_usage_object(value: Option<&Value>) -> AssistantUsage {
    let mut usage = AssistantUsage::default();
    let Some(record) = value.and_then(Value::as_object) else {
        return usage;
    };
    usage.input_tokens = pick_u64(
        record,
        &[
            "inputTokens",
            "input_tokens",
            "promptTokens",
            "prompt_tokens",
        ],
    );
    usage.output_tokens = [
        "outputTokens",
        "output_tokens",
        "completionTokens",
        "completion_tokens",
    ]
    .iter()
    .find_map(|key| record.get(*key).and_then(Value::as_u64));
    usage.cache_read_input_tokens =
        pick_u64(record, &["cacheReadInputTokens", "cache_read_input_tokens"])
            .max(pick_nested_u64(
                record,
                "promptTokensDetails",
                &["cachedTokens"],
            ))
            .max(pick_nested_u64(
                record,
                "prompt_tokens_details",
                &["cached_tokens"],
            ));
    usage.cache_creation_input_tokens = pick_u64(
        record,
        &[
            "cacheCreationInputTokens",
            "cache_creation_input_tokens",
            "cacheCreationTokens",
            "cache_creation_tokens",
            "cachedTokensCreated",
            "cached_tokens_created",
        ],
    );
    usage.total_tokens = pick_u64(record, &["totalTokens", "total_tokens", "total"]);
    usage.credits = number_field(record, "credits");
    usage.model = string_field(record, "model");
    usage
}

fn merge_fallback(target: &mut AssistantUsage, fallback: AssistantUsage) {
    if target.input_tokens == 0 {
        target.input_tokens = fallback.input_tokens;
    }
    if target.output_tokens.is_none() {
        target.output_tokens = fallback.output_tokens;
    }
    if target.cache_creation_input_tokens == 0 {
        target.cache_creation_input_tokens = fallback.cache_creation_input_tokens;
    }
    if target.cache_read_input_tokens == 0 {
        target.cache_read_input_tokens = fallback.cache_read_input_tokens;
    }
    if target.total_tokens == 0 {
        target.total_tokens = fallback.total_tokens;
    }
    if target.credits <= 0.0 {
        target.credits = fallback.credits;
    }
    if target.model.is_none() {
        target.model = fallback.model;
    }
}

fn has_signal(usage: &AssistantUsage) -> bool {
    usage.input_tokens > 0
        || usage.output_tokens.unwrap_or_default() > 0
        || usage.cache_creation_input_tokens > 0
        || usage.cache_read_input_tokens > 0
        || usage.total_tokens > 0
        || usage.credits > 0.0
}

fn message_timestamp(message: &serde_json::Map<String, Value>) -> Option<crate::TimestampMs> {
    timestamp_value(message.get("timestamp"))
        .or_else(|| timestamp_value(message.get("createdAt")))
        .or_else(|| {
            object_field(message, "metadata")
                .and_then(|metadata| timestamp_value(metadata.get("timestamp")))
        })
}

fn parse_codebuff_chat_id_timestamp(chat_id: &str) -> Option<crate::TimestampMs> {
    let (date, time) = chat_id.split_once('T')?;
    let mut time = time.to_string();
    for _ in 0..2 {
        if let Some(index) = time.find('-') {
            time.replace_range(index..=index, ":");
        }
    }
    parse_ts_timestamp(&format!("{date}T{time}"))
}

fn timestamp_value(value: Option<&Value>) -> Option<crate::TimestampMs> {
    match value? {
        Value::String(value) => parse_ts_timestamp(value),
        Value::Number(value) => {
            ccusage_adapter_common::timestamps::epoch_secs_or_millis(value.as_i64()?)
        }
        Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_) => None,
    }
}

fn dedup_key(
    message: &serde_json::Map<String, Value>,
    session_id: &str,
    timestamp: crate::TimestampMs,
    model: &str,
    usage: TokenUsageRaw,
    extra_total_tokens: u64,
    ordinal: usize,
) -> String {
    if let Some(id) = string_field(message, "id") {
        return format!("codebuff:{session_id}:{id}");
    }
    format!(
        "codebuff:{session_id}:{}:{model}:{ordinal}:{}:{}:{}:{}:{}",
        format_rfc3339_millis(timestamp),
        usage.input_tokens,
        usage.output_tokens,
        usage.cache_read_input_tokens,
        usage.cache_creation_input_tokens,
        extra_total_tokens
    )
}

fn infer_provider(model: &str) -> &'static str {
    let model = model.to_ascii_lowercase();
    if model.starts_with("claude-")
        || model.starts_with("anthropic/")
        || model.starts_with("anthropic.")
    {
        "anthropic"
    } else if model.starts_with("gpt-")
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
        || model.starts_with("openai/")
    {
        "openai"
    } else if model.starts_with("gemini") || model.starts_with("google/") {
        "google"
    } else if model.starts_with("grok") || model.starts_with("xai/") {
        "xai"
    } else if model.starts_with("openrouter/") {
        "openrouter"
    } else {
        "unknown"
    }
}

pub(super) fn price_codebuff(
    entry: &CodebuffEntry,
    pricing: PriceTable<'_>,
) -> (f64, Option<String>, bool) {
    let candidates = codebuff_model_candidates(entry);
    let candidates: Vec<_> = candidates
        .iter()
        .map(|name| (name.as_str(), MatchKind::Fuzzy))
        .collect();
    let entry_pricing = EntryPricing {
        granularity: UsageGranularity::Request,
        usage: entry.usage,
        reasoning_tokens: 0,
        reported_model: Some(&entry.model),
        candidates: &candidates,
        stored_cost: None,
        timestamp: Some(entry.timestamp),
    };
    // NOTE(2026-11-20, #2332): Codebuff bills every request through its own service. Every vendor it records is therefore a third party and must not inherit DeepSeek's schedule.
    let priced = price_entry_on_route(entry_pricing, BillingRoute::Foreign, pricing);
    (
        priced.cost,
        priced
            .missing_pricing
            .then(|| ccusage_core::model_aliases::resolve_model_name(&entry.model).into_owned()),
        priced.estimated,
    )
}

fn codebuff_model_candidates(entry: &CodebuffEntry) -> Vec<String> {
    let mut candidates = vec![entry.model.clone()];
    if entry.provider != "unknown" && !entry.model.starts_with(&format!("{}/", entry.provider)) {
        candidates.push(format!("{}/{}", entry.provider, entry.model));
    }
    candidates
}

fn object_field<'a>(
    record: &'a serde_json::Map<String, Value>,
    key: &str,
) -> Option<&'a serde_json::Map<String, Value>> {
    record.get(key).and_then(Value::as_object)
}

fn string_field(record: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    let value = record.get(key)?.as_str()?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn number_field(record: &serde_json::Map<String, Value>, key: &str) -> f64 {
    record
        .get(key)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(0.0)
}

fn pick_u64(record: &serde_json::Map<String, Value>, keys: &[&str]) -> u64 {
    keys.iter()
        .filter_map(|key| record.get(*key))
        .find_map(|value| value.as_u64().filter(|value| *value > 0))
        .unwrap_or(0)
}

fn pick_nested_u64(record: &serde_json::Map<String, Value>, key: &str, keys: &[&str]) -> u64 {
    object_field(record, key).map_or(0, |nested| pick_u64(nested, keys))
}

#[cfg(test)]
mod pricing_identity_tests {
    use super::*;

    #[test]
    fn adapter_pricing_identity() {
        let entry = CodebuffEntry {
            timestamp: crate::parse_ts_timestamp("2026-11-17T10:00:00Z").unwrap(),

            session_id: "s".into(),
            model: "gpt-5.2".into(),
            provider: "openai".into(),
            credits: 0.0,
            usage: TokenUsageRaw::default(),
            extra_total_tokens: 0,
            reported_output_tokens: 0,
            dedup_key: String::new(),
        };
        assert_eq!(
            codebuff_model_candidates(&entry),
            ["gpt-5.2", "openai/gpt-5.2"]
        );
    }

    #[test]
    fn prices_a_foreign_router_without_the_deepseek_schedule() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        for (provider, expected) in [("unknown", 0.0015), ("openrouter", 0.0015)] {
            let entry = CodebuffEntry {
                timestamp: crate::parse_ts_timestamp("2026-11-17T10:00:00Z").unwrap(),

                session_id: "s".into(),
                model: "deepseek-v4-flash".into(),
                provider: provider.into(),
                credits: 0.0,
                usage: TokenUsageRaw {
                    input_tokens: 1_000,
                    output_tokens: 1_000,
                    ..TokenUsageRaw::default()
                },
                extra_total_tokens: 0,
                reported_output_tokens: 1_000,
                dedup_key: String::new(),
            };

            let (cost, missing, _) = price_codebuff(&entry, PriceTable::Calculate(&pricing));
            assert!((cost - expected).abs() < 1e-12, "{provider}: {cost}");
            assert_eq!(missing, None, "{provider}");
        }
    }
}
