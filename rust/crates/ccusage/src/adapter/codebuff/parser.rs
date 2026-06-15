use std::{fs, path::Path};

use serde_json::Value;

use crate::{
    PricingMap, Result, TokenUsageRaw, apply_total_token_fallback, calculate_cost_for_usage,
    cli::CostMode, format_rfc3339_millis, missing_pricing_model_for_candidates, parse_ts_timestamp,
};

const DEFAULT_CODEBUFF_MODEL: &str = "codebuff-unknown";

#[derive(Clone, Default)]
pub(super) struct AssistantUsage {
    pub(super) model: Option<String>,
    pub(super) credits: f64,
    pub(super) input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) cache_creation_input_tokens: u64,
    pub(super) cache_read_input_tokens: u64,
    pub(super) extra_total_tokens: u64,
}

pub(super) struct CodebuffEntry {
    pub(super) timestamp: crate::TimestampMs,
    pub(super) timestamp_text: String,
    pub(super) session_id: String,
    pub(super) model: String,
    provider: String,
    pub(super) credits: f64,
    pub(super) usage: TokenUsageRaw,
    pub(super) extra_total_tokens: u64,
    pub(super) dedup_key: String,
}

struct CodebuffContext {
    chat_id: String,
    session_id: String,
}

pub(super) fn load_chat_file(path: &Path) -> Result<Vec<CodebuffEntry>> {
    let content = fs::read(path)?;
    let Ok(messages) = serde_json::from_slice::<Vec<Value>>(&content) else {
        return Ok(Vec::new());
    };
    let context = derive_context(path);
    let chat_timestamp = parse_codebuff_chat_id_timestamp(&context.chat_id);
    let file_timestamp = file_modified_timestamp(path).unwrap_or(crate::TimestampMs::UNIX_EPOCH);
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
        let timestamp = message_timestamp(message)
            .or(chat_timestamp)
            .unwrap_or(file_timestamp);
        let model = usage
            .model
            .clone()
            .unwrap_or_else(|| DEFAULT_CODEBUFF_MODEL.to_string());
        let dedup_key = dedup_key(
            message,
            &context.session_id,
            timestamp,
            &model,
            &usage,
            ordinal,
        );
        entries.push(CodebuffEntry {
            timestamp,
            timestamp_text: format_rfc3339_millis(timestamp),
            session_id: context.session_id.clone(),
            provider: infer_provider(&model).to_string(),
            model,
            credits: usage.credits,
            usage: TokenUsageRaw {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_creation_input_tokens: usage.cache_creation_input_tokens,
                cache_read_input_tokens: usage.cache_read_input_tokens,
                speed: None,
                cache_creation: None,
            },
            extra_total_tokens: usage.extra_total_tokens,
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

pub(super) fn parse_usage_object(value: Option<&Value>) -> AssistantUsage {
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
    usage.output_tokens = pick_u64(
        record,
        &[
            "outputTokens",
            "output_tokens",
            "completionTokens",
            "completion_tokens",
        ],
    );
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
    let total_tokens = pick_u64(record, &["totalTokens", "total_tokens", "total"]);
    let raw_usage = TokenUsageRaw {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_creation_input_tokens: usage.cache_creation_input_tokens,
        cache_read_input_tokens: usage.cache_read_input_tokens,
        speed: None,
        cache_creation: None,
    };
    let (raw_usage, extra_total_tokens) =
        apply_total_token_fallback(raw_usage, usage.extra_total_tokens, total_tokens);
    usage.input_tokens = raw_usage.input_tokens;
    usage.output_tokens = raw_usage.output_tokens;
    usage.cache_creation_input_tokens = raw_usage.cache_creation_input_tokens;
    usage.cache_read_input_tokens = raw_usage.cache_read_input_tokens;
    usage.extra_total_tokens = extra_total_tokens;
    usage.credits = number_field(record, "credits");
    usage.model = string_field(record, "model");
    usage
}

fn merge_fallback(target: &mut AssistantUsage, fallback: AssistantUsage) {
    if target.input_tokens == 0 {
        target.input_tokens = fallback.input_tokens;
    }
    if target.output_tokens == 0 {
        target.output_tokens = fallback.output_tokens;
    }
    if target.cache_creation_input_tokens == 0 {
        target.cache_creation_input_tokens = fallback.cache_creation_input_tokens;
    }
    if target.cache_read_input_tokens == 0 {
        target.cache_read_input_tokens = fallback.cache_read_input_tokens;
    }
    if target.extra_total_tokens == 0 {
        target.extra_total_tokens = fallback.extra_total_tokens;
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
        || usage.output_tokens > 0
        || usage.cache_creation_input_tokens > 0
        || usage.cache_read_input_tokens > 0
        || usage.extra_total_tokens > 0
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
            let raw = value.as_i64()?;
            let millis = if raw < 10_000_000_000 {
                raw.checked_mul(1_000)?
            } else {
                raw
            };
            (millis > 0).then(|| crate::TimestampMs::from_millis(millis))
        }
        _ => None,
    }
}

fn file_modified_timestamp(path: &Path) -> Option<crate::TimestampMs> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let millis = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis()
        .min(i64::MAX as u128) as i64;
    Some(crate::TimestampMs::from_millis(millis))
}

fn dedup_key(
    message: &serde_json::Map<String, Value>,
    session_id: &str,
    timestamp: crate::TimestampMs,
    model: &str,
    usage: &AssistantUsage,
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
        usage.extra_total_tokens
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

pub(super) fn calculate_codebuff_cost(entry: &CodebuffEntry, pricing: &PricingMap) -> f64 {
    let usage = TokenUsageRaw {
        output_tokens: entry
            .usage
            .output_tokens
            .saturating_add(entry.extra_total_tokens),
        cache_creation: None,
        ..entry.usage
    };
    let raw = calculate_cost_for_usage(
        Some(&entry.model),
        usage,
        None,
        CostMode::Calculate,
        Some(pricing),
    );
    if raw > 0.0
        || entry.provider == "unknown"
        || entry.model.starts_with(&format!("{}/", entry.provider))
    {
        return raw;
    }
    calculate_cost_for_usage(
        Some(&format!("{}/{}", entry.provider, entry.model)),
        usage,
        None,
        CostMode::Calculate,
        Some(pricing),
    )
}

pub(super) fn missing_codebuff_pricing(
    entry: &CodebuffEntry,
    pricing: &PricingMap,
) -> Option<String> {
    let usage = TokenUsageRaw {
        output_tokens: entry
            .usage
            .output_tokens
            .saturating_add(entry.extra_total_tokens),
        cache_creation: None,
        ..entry.usage
    };
    let mut candidates = vec![entry.model.clone()];
    if entry.provider != "unknown" && !entry.model.starts_with(&format!("{}/", entry.provider)) {
        candidates.push(format!("{}/{}", entry.provider, entry.model));
    }
    missing_pricing_model_for_candidates(
        &entry.model,
        candidates,
        crate::total_usage_tokens(usage),
        Some(pricing),
    )
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
