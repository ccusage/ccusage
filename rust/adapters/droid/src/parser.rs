use std::{collections::HashSet, fs, io, path::Path};

use serde_json::Value;

use crate::{
    PricingMap, Result, TokenUsageRaw, apply_total_token_fallback, calculate_cost_for_usage,
    cli::CostMode, format_rfc3339_millis, json_value_u64, missing_pricing_model_for_candidates,
    parse_ts_timestamp,
};

#[derive(Clone)]
pub(super) struct DroidEntry {
    pub(super) timestamp: crate::TimestampMs,
    pub(super) timestamp_text: String,
    pub(super) session_id: String,
    pub(super) model: String,
    provider: String,
    pub(super) usage: TokenUsageRaw,
    pub(super) reasoning_tokens: u64,
}

#[derive(Default)]
pub(super) struct DroidTokenUsage {
    pub(super) input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) cache_creation_tokens: u64,
    pub(super) cache_read_tokens: u64,
    pub(super) thinking_tokens: u64,
}

pub(super) fn load_settings_file(path: &Path) -> Result<Option<DroidEntry>> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let value = serde_json::from_str::<Value>(&content).map_err(|error| {
        crate::cli_error(format!(
            "failed to parse Droid settings {}: {error}",
            path.display()
        ))
    })?;
    let Some(settings) = value.as_object() else {
        return Ok(None);
    };
    let Some(usage) = parse_token_usage(settings.get("tokenUsage")) else {
        return Ok(None);
    };
    let provider = normalize_droid_provider(string_field(settings, "providerLock").as_deref());
    let model = if let Some(model) = string_field(settings, "model") {
        normalize_droid_model_name(&model)
    } else {
        extract_model_from_sidecar_jsonl(path)?
            .unwrap_or_else(|| default_model_from_provider(&provider).to_string())
    };
    let model = if model.is_empty() {
        default_model_from_provider(&provider).to_string()
    } else {
        model
    };
    let provider = if provider == "unknown" {
        infer_droid_provider_from_model(&model).to_string()
    } else {
        provider
    };
    let Some((timestamp, timestamp_text)) = settings_timestamp(settings, path) else {
        return Ok(None);
    };
    let session_id = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix(".settings.json"))
        .unwrap_or("unknown")
        .to_string();
    Ok(Some(DroidEntry {
        timestamp,
        timestamp_text,
        session_id,
        model,
        provider,
        usage: TokenUsageRaw {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_creation_input_tokens: usage.cache_creation_tokens,
            cache_read_input_tokens: usage.cache_read_tokens,
            speed: None,
            cache_creation: None,
        },
        reasoning_tokens: usage.thinking_tokens,
    }))
}

pub(super) fn parse_token_usage(value: Option<&Value>) -> Option<DroidTokenUsage> {
    let usage = value?.as_object()?;
    let raw_usage = TokenUsageRaw {
        input_tokens: json_value_u64(usage.get("inputTokens")),
        output_tokens: json_value_u64(usage.get("outputTokens")),
        cache_creation_input_tokens: json_value_u64(usage.get("cacheCreationTokens")),
        cache_read_input_tokens: json_value_u64(usage.get("cacheReadTokens")),
        speed: None,
        cache_creation: None,
    };
    let thinking_tokens = json_value_u64(usage.get("thinkingTokens"));
    let total_tokens = json_value_u64(usage.get("totalTokens"));
    let (raw_usage, thinking_tokens) =
        apply_total_token_fallback(raw_usage, thinking_tokens, total_tokens);
    let tokens = DroidTokenUsage {
        input_tokens: raw_usage.input_tokens,
        output_tokens: raw_usage.output_tokens,
        cache_creation_tokens: raw_usage.cache_creation_input_tokens,
        cache_read_tokens: raw_usage.cache_read_input_tokens,
        thinking_tokens,
    };
    (tokens.input_tokens
        + tokens.output_tokens
        + tokens.cache_creation_tokens
        + tokens.cache_read_tokens
        + tokens.thinking_tokens
        > 0)
    .then_some(tokens)
}

fn settings_timestamp(
    settings: &serde_json::Map<String, Value>,
    path: &Path,
) -> Option<(crate::TimestampMs, String)> {
    if let Some(timestamp_text) = string_field(settings, "providerLockTimestamp")
        && let Some(timestamp) = parse_ts_timestamp(&timestamp_text)
    {
        return Some((timestamp, format_rfc3339_millis(timestamp)));
    }
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let millis = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis()
        .min(i64::MAX as u128) as i64;
    let timestamp = crate::TimestampMs::from_millis(millis);
    Some((timestamp, format_rfc3339_millis(timestamp)))
}

pub(super) fn calculate_droid_cost(entry: &DroidEntry, pricing: &PricingMap) -> f64 {
    let usage = TokenUsageRaw {
        output_tokens: entry.usage.output_tokens + entry.reasoning_tokens,
        cache_creation: None,
        ..entry.usage
    };
    for candidate in droid_model_candidates(entry) {
        let cost = calculate_cost_for_usage(
            Some(&candidate),
            usage,
            None,
            CostMode::Calculate,
            Some(pricing),
        );
        if cost > 0.0 {
            return cost;
        }
    }
    0.0
}

pub(super) fn missing_droid_pricing(entry: &DroidEntry, pricing: &PricingMap) -> Option<String> {
    let usage = TokenUsageRaw {
        output_tokens: entry.usage.output_tokens + entry.reasoning_tokens,
        cache_creation: None,
        ..entry.usage
    };
    missing_pricing_model_for_candidates(
        &entry.model,
        droid_model_candidates(entry),
        crate::total_usage_tokens(usage),
        Some(pricing),
    )
}

fn droid_model_candidates(entry: &DroidEntry) -> Vec<String> {
    let mut candidates = vec![entry.model.clone()];
    for prefix in provider_prefixes(&entry.provider) {
        candidates.push(format!("{prefix}{}", entry.model));
    }
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|candidate| seen.insert(candidate.clone()))
        .collect()
}

fn provider_prefixes(provider: &str) -> Vec<String> {
    match provider {
        "anthropic" => vec![
            "anthropic/".to_string(),
            "openrouter/anthropic/".to_string(),
        ],
        "openai" => vec!["openai/".to_string(), "openrouter/openai/".to_string()],
        "google" => vec![
            "google/".to_string(),
            "vertex_ai/".to_string(),
            "openrouter/google/".to_string(),
        ],
        "xai" => vec!["xai/".to_string(), "openrouter/x-ai/".to_string()],
        "unknown" => Vec::new(),
        provider => vec![format!("{provider}/"), format!("openrouter/{provider}/")],
    }
}

fn string_field(record: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    let value = record.get(key)?.as_str()?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

pub fn normalize_droid_model_name(model: &str) -> String {
    let raw = model.strip_prefix("custom:").unwrap_or(model);
    let mut without_brackets = String::new();
    let mut bracket_depth = 0_u32;
    for ch in raw.chars() {
        match ch {
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            _ if bracket_depth == 0 => without_brackets.push(ch),
            _ => {}
        }
    }
    let lower = without_brackets
        .trim()
        .trim_end_matches('-')
        .to_ascii_lowercase();
    let mut normalized = String::new();
    let mut previous_dash = false;
    for ch in lower.chars() {
        let next = if ch == '.' || ch.is_whitespace() || ch == '-' {
            '-'
        } else {
            ch
        };
        if next == '-' {
            if !previous_dash {
                normalized.push('-');
                previous_dash = true;
            }
        } else {
            normalized.push(next);
            previous_dash = false;
        }
    }
    normalized.trim_matches('-').to_string()
}

fn normalize_droid_provider(value: Option<&str>) -> String {
    let Some(value) = value else {
        return "unknown".to_string();
    };
    let normalized = value.trim().to_ascii_lowercase().replace('-', "_");
    match normalized.as_str() {
        "" => "unknown".to_string(),
        "claude" | "anthropic" => "anthropic".to_string(),
        "openai" => "openai".to_string(),
        "google" | "google_ai" | "gemini" | "vertex" | "vertex_ai" => "google".to_string(),
        "xai" | "x_ai" | "grok" => "xai".to_string(),
        value => value.to_string(),
    }
}

fn infer_droid_provider_from_model(model: &str) -> &'static str {
    if model.contains("claude")
        || model.contains("opus")
        || model.contains("sonnet")
        || model.contains("haiku")
    {
        "anthropic"
    } else if model.starts_with("gpt-")
        || model.contains("-gpt-")
        || model.contains("chatgpt")
        || model.starts_with('o') && model.as_bytes().get(1).is_some_and(u8::is_ascii_digit)
    {
        "openai"
    } else if model.contains("gemini") {
        "google"
    } else if model.contains("grok") {
        "xai"
    } else {
        "unknown"
    }
}

fn default_model_from_provider(provider: &str) -> &'static str {
    match provider {
        "anthropic" => "claude-unknown",
        "openai" => "gpt-unknown",
        "google" => "gemini-unknown",
        "xai" => "grok-unknown",
        "unknown" => "unknown",
        _ => "unknown",
    }
}

fn extract_model_from_sidecar_jsonl(settings_path: &Path) -> Result<Option<String>> {
    let Some(file_name) = settings_path.file_name().and_then(|name| name.to_str()) else {
        return Ok(None);
    };
    let Some(prefix) = file_name.strip_suffix(".settings.json") else {
        return Ok(None);
    };
    let sidecar = settings_path.with_file_name(format!("{prefix}.jsonl"));
    let Ok(content) = fs::read_to_string(sidecar) else {
        return Ok(None);
    };
    for line in content.lines().take(500) {
        if let Some(model) = extract_droid_model_from_line(line) {
            return Ok(Some(model));
        }
    }
    Ok(None)
}

fn extract_droid_model_from_line(line: &str) -> Option<String> {
    let tail = line.split_once("Model:")?.1;
    let raw = tail
        .split(['"', '\\', '['])
        .next()
        .unwrap_or_default()
        .trim();
    if raw.is_empty() {
        return None;
    }
    let normalized = normalize_droid_model_name(raw);
    (!normalized.is_empty()).then_some(normalized)
}
