use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
    sync::Arc,
};

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;

use crate::{
    LoadedEntry, PricingMap, Result, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    apply_total_token_fallback, calculate_cost_for_usage_at, cli::CostMode, format_date_tz,
    format_rfc3339_millis,
};
use ccusage_adapter_common::jsonl;

#[derive(Debug, Clone)]
pub(super) struct DshUsageEntry {
    pub(super) session_id: String,
    pub(super) message_id: String,
    cwd: String,
    provider: String,
    model: String,
    display_model: String,
    pricing_candidates: Vec<String>,
    usage: TokenUsageRaw,
    extra_total_tokens: u64,
    timestamp: TimestampMs,
    pub(super) format_version: u64,
}

#[derive(Debug, Default, Deserialize)]
struct SessionLine {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    r#type: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    id: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    cwd: Option<String>,
    version: Option<u64>,
    #[serde(default, deserialize_with = "jsonl::lenient_i64")]
    time: Option<i64>,
    data: Option<SessionData>,
}

#[derive(Debug, Deserialize)]
struct SessionData {
    message: Option<SessionMessage>,
    usage: Option<SessionUsage>,
}

#[derive(Debug, Deserialize)]
struct SessionMessage {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    id: Option<String>,
    source: Option<SessionSource>,
}

#[derive(Debug, Deserialize)]
struct SessionSource {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    provider: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    model: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionUsage {
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    input_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    output_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cache_read_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cache_write_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    total_tokens: u64,
}

pub(super) fn read_session_file(path: &Path, filename_version: u64) -> Result<Vec<DshUsageEntry>> {
    let file = File::open(path)?;
    let mut reader: Box<dyn BufRead> = if path
        .extension()
        .is_some_and(|extension| extension == "zstd")
    {
        Box::new(BufReader::new(zstd::stream::read::Decoder::new(file)?))
    } else {
        Box::new(BufReader::new(file))
    };
    let fallback_timestamp = modified_timestamp(path);
    let mut line = Vec::new();
    let mut session: Option<(String, String, u64)> = None;
    let mut entries = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) if entries.is_empty() => return Err(error.into()),
            Err(_) => break,
        }
        let Ok(record) = serde_json::from_slice::<SessionLine>(&line) else {
            continue;
        };
        if record.r#type.as_deref() == Some("session") {
            let Some(id) = record.id else {
                session = None;
                continue;
            };
            let cwd = record.cwd.unwrap_or_else(|| "DeepSeek Harness".to_string());
            let version = record.version.unwrap_or(filename_version);
            session = Some((id, cwd, version));
            continue;
        }
        if record.r#type.as_deref() != Some("assistant/message") {
            continue;
        }
        let Some((session_id, cwd, format_version)) = session.as_ref() else {
            continue;
        };
        let Some(data) = record.data else {
            continue;
        };
        let (Some(message), Some(raw_usage)) = (data.message, data.usage) else {
            continue;
        };
        let (Some(message_id), Some(source)) = (message.id, message.source) else {
            continue;
        };
        let (Some(provider), Some(model)) = (source.provider, source.model) else {
            continue;
        };
        let usage = TokenUsageRaw {
            input_tokens: raw_usage.input_tokens,
            output_tokens: raw_usage.output_tokens,
            cache_creation_input_tokens: raw_usage.cache_write_tokens,
            cache_read_input_tokens: raw_usage.cache_read_tokens,
            speed: None,
            cache_creation: None,
        };
        let (usage, extra_total_tokens) =
            apply_total_token_fallback(usage, 0, raw_usage.total_tokens);
        if crate::total_usage_tokens(usage) + extra_total_tokens == 0 {
            continue;
        }
        let (display_model, pricing_candidates) = provider_model_candidates(&provider, &model);
        entries.push(DshUsageEntry {
            session_id: session_id.clone(),
            message_id,
            cwd: cwd.clone(),
            display_model,
            pricing_candidates,
            provider,
            model,
            usage,
            extra_total_tokens,
            timestamp: record
                .time
                .map(TimestampMs::from_millis)
                .unwrap_or(fallback_timestamp),
            format_version: *format_version,
        });
    }
    Ok(entries)
}

fn modified_timestamp(path: &Path) -> TimestampMs {
    File::open(path)
        .and_then(|file| file.metadata())
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .map(TimestampMs::from_millis)
        .unwrap_or(TimestampMs::UNIX_EPOCH)
}

fn canonical_provider(provider: &str) -> String {
    match provider.trim().to_ascii_lowercase().as_str() {
        "together" | "together-ai" | "together_ai" => "together_ai".to_string(),
        value => value.to_string(),
    }
}

fn provider_model_candidates(provider: &str, model: &str) -> (String, Vec<String>) {
    if provider.trim().eq_ignore_ascii_case("deepseek") {
        (model.to_string(), vec![model.to_string()])
    } else {
        let display_model = format!("{}/{}", canonical_provider(provider), model);
        let mut candidates = vec![display_model.clone()];
        if let Some(canonical) = canonical_deepseek_pricing_model(model)
            && !candidates.contains(&canonical)
        {
            candidates.push(canonical);
        }
        (display_model, candidates)
    }
}

fn canonical_deepseek_pricing_model(model: &str) -> Option<String> {
    let model = model.trim();
    let lower = model.to_ascii_lowercase();
    let canonical = match lower.as_str() {
        "deepseek-v4.1-flash:1017-cloud" | "deepseek-v4.1-flash:1017" => {
            "deepseek-ai/DeepSeek-V4.1-Flash-1017"
        }
        "deepseek-v4.1-flash" => "deepseek-ai/deepseek-v4.1-flash",
        _ if lower.starts_with("deepseek-ai/") => return Some(model.to_string()),
        _ => return None,
    };
    Some(canonical.to_string())
}

pub(super) fn to_loaded_entry(
    entry: DshUsageEntry,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: &PricingMap,
) -> LoadedEntry {
    let timestamp_text = format_rfc3339_millis(entry.timestamp);
    let cost = calculate_cost(&entry, mode, pricing);
    let missing_pricing_model = missing_pricing_model(&entry, mode, pricing);
    let model = entry.display_model.clone();
    let data = UsageEntry {
        session_id: Some(entry.session_id.clone()),
        timestamp: timestamp_text,
        version: Some(entry.format_version.to_string()),
        message: UsageMessage {
            usage: entry.usage,
            model: Some(model.clone()),
            id: Some(entry.message_id),
        },
        cost_usd: None,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };
    LoadedEntry {
        date: format_date_tz(entry.timestamp, tz),
        timestamp: entry.timestamp,
        project: Arc::from("dsh"),
        session_id: Arc::from(entry.session_id),
        project_path: Arc::from(entry.cwd),
        cost,
        extra_total_tokens: entry.extra_total_tokens,
        credits: None,
        message_count: None,
        model: Some(model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        stored_cost: None,
        data,
    }
}

fn calculate_cost(entry: &DshUsageEntry, mode: CostMode, pricing: &PricingMap) -> f64 {
    if mode == CostMode::Display {
        return 0.0;
    }
    for candidate in &entry.pricing_candidates {
        let is_direct = entry.provider.trim().eq_ignore_ascii_case("deepseek");
        if is_direct && candidate == &entry.model {
            return calculate_cost_for_usage_at(
                Some(candidate),
                entry.usage,
                None,
                Some(entry.timestamp),
                mode,
                Some(pricing),
            );
        }
        if pricing.find_exact_with_fallback(candidate).is_some() {
            return calculate_cost_for_usage_at(
                Some(candidate),
                entry.usage,
                None,
                Some(entry.timestamp),
                mode,
                Some(pricing),
            );
        }
    }
    0.0
}

fn missing_pricing_model(
    entry: &DshUsageEntry,
    mode: CostMode,
    pricing: &PricingMap,
) -> Option<String> {
    if mode == CostMode::Display || crate::total_usage_tokens(entry.usage) == 0 {
        return None;
    }
    let has_pricing = entry.pricing_candidates.iter().any(|candidate| {
        if entry.provider.trim().eq_ignore_ascii_case("deepseek") && candidate == &entry.model {
            pricing.find_at(candidate, entry.timestamp).is_some()
        } else {
            pricing.find_exact_with_fallback(candidate).is_some()
        }
    });
    (!has_pricing).then(|| entry.display_model.clone())
}

#[cfg(test)]
mod tests {
    use super::{canonical_provider, provider_model_candidates};

    #[test]
    fn keeps_provider_routes_qualified_for_pricing_and_display() {
        assert_eq!(
            provider_model_candidates("ollama", "deepseek-v4.1-flash:1017-cloud"),
            (
                "ollama/deepseek-v4.1-flash:1017-cloud".to_string(),
                vec![
                    "ollama/deepseek-v4.1-flash:1017-cloud".to_string(),
                    "deepseek-ai/DeepSeek-V4.1-Flash-1017".to_string(),
                ]
            )
        );
        assert_eq!(
            provider_model_candidates("together", "deepseek-ai/DeepSeek-V4-Flash-0731"),
            (
                "together_ai/deepseek-ai/DeepSeek-V4-Flash-0731".to_string(),
                vec![
                    "together_ai/deepseek-ai/DeepSeek-V4-Flash-0731".to_string(),
                    "deepseek-ai/DeepSeek-V4-Flash-0731".to_string(),
                ]
            )
        );
        assert_eq!(
            provider_model_candidates("deepseek", "deepseek-v4.1-flash"),
            (
                "deepseek-v4.1-flash".to_string(),
                vec!["deepseek-v4.1-flash".to_string()]
            )
        );
        assert_eq!(canonical_provider("together-ai"), "together_ai");
        assert_eq!(
            provider_model_candidates("ollama", "DeepSeek-V4-Flash-0731"),
            (
                "ollama/DeepSeek-V4-Flash-0731".to_string(),
                vec!["ollama/DeepSeek-V4-Flash-0731".to_string()]
            ),
            "dated model releases must not be rewritten to a different generation"
        );
    }
}
