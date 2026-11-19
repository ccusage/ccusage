use ccusage_core::cost::PriceTable;
use ccusage_core::{
    cost::{EntryPricing, price_entry},
    pricing::MatchKind,
};
use std::{fs, path::Path};

use ccusage_adapter_common::units::{UnitError, read_json};
use serde_json::Value;

use crate::{
    Result, TokenUsageRaw, apply_total_token_fallback, format_rfc3339_millis, json_value_u64,
    parse_ts_timestamp,
};

#[derive(Clone)]
pub(super) struct DroidEntry {
    pub(super) timestamp: crate::TimestampMs,
    pub(super) timestamp_text: String,
    pricing_timestamp: Option<crate::TimestampMs>,
    pub(super) session_id: String,
    pub(super) model: String,
    provider: String,
    provider_locked: bool,
    pub(super) usage: TokenUsageRaw,
    pub(super) reasoning_tokens: u64,
}

#[derive(Default)]
pub(super) struct DroidTokenUsage {
    input_tokens: u64,
    pub(super) output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    pub(super) thinking_tokens: u64,
}

pub(super) fn load_settings_file(
    path: &Path,
) -> std::result::Result<Option<DroidEntry>, UnitError> {
    let value = read_json::<Value>(path)?;
    let Some(settings) = value.as_object() else {
        return Ok(None);
    };
    let Some(usage) = parse_token_usage(settings.get("tokenUsage")) else {
        return Ok(None);
    };
    let provider = normalize_droid_provider(string_field(settings, "providerLock").as_deref());
    let provider_locked = provider != "unknown";
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
    let (timestamp, timestamp_text, pricing_timestamp) = settings_timestamp(settings, path)
        .ok_or_else(|| {
            UnitError::new(
                ccusage_core::SkipKind::InvalidTimestamp,
                "Droid settings have no valid timestamp or file modification time",
            )
        })?;
    let session_id = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix(".settings.json"))
        .unwrap_or("unknown")
        .to_string();
    Ok(Some(DroidEntry {
        timestamp,
        timestamp_text,
        pricing_timestamp,
        session_id,
        model,
        provider,
        provider_locked,
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
) -> Option<(crate::TimestampMs, String, Option<crate::TimestampMs>)> {
    if let Some(timestamp_text) = string_field(settings, "providerLockTimestamp")
        && let Some(timestamp) = parse_ts_timestamp(&timestamp_text)
    {
        return Some((timestamp, format_rfc3339_millis(timestamp), Some(timestamp)));
    }
    let timestamp = ccusage_adapter_common::timestamps::file_mtime(path)?;
    Some((timestamp, format_rfc3339_millis(timestamp), None))
}

pub(super) fn price_droid(entry: &DroidEntry, pricing: PriceTable<'_>) -> (f64, Option<String>) {
    let qualified: Vec<_> = provider_prefixes(&entry.provider)
        .into_iter()
        .map(|prefix| format!("{prefix}{}", entry.model))
        .collect();
    let map = pricing.as_map();
    let locked = qualified.iter().filter(|model| {
        entry.provider_locked && map.is_some_and(|map| map.has_primary_price(model))
    });
    let preserve_locked_plan = locked.clone().next().is_some()
        && map.is_some_and(|map| map.has_base_rate_override_with_long_context(&entry.model));
    let candidates: Vec<_> = locked
        .map(|name| (name.as_str(), MatchKind::Exact))
        .chain(
            (!preserve_locked_plan)
                .then_some((entry.model.as_str(), MatchKind::Fuzzy))
                .into_iter()
                .chain(
                    qualified
                        .iter()
                        .filter(|_| !preserve_locked_plan)
                        .map(|name| (name.as_str(), MatchKind::Fuzzy)),
                ),
        )
        .collect();
    let priced = price_entry(
        EntryPricing {
            usage: entry.usage,
            reasoning_tokens: 0,
            residual_tokens: entry.reasoning_tokens,
            reported_model: (!preserve_locked_plan).then_some(entry.model.as_str()),
            candidates: &candidates,
            stored_cost: None,
            timestamp: entry.pricing_timestamp,
        },
        pricing,
    );
    (
        priced.cost,
        priced
            .missing_pricing
            .then(|| ccusage_core::model_aliases::resolve_model_name(&entry.model).into_owned()),
    )
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
    let raw = model.trim_prefix("custom:");
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

#[cfg(test)]
mod tests {
    #[test]
    fn zero_rates_stop_candidate_fallback() {
        let entry = DroidEntry {
            timestamp: crate::parse_ts_timestamp("2026-05-19T00:00:00Z").unwrap(),
            timestamp_text: String::new(),
            pricing_timestamp: None,
            session_id: "s".into(),
            model: "sample".into(),
            provider: "vendor".into(),
            provider_locked: false,
            usage: TokenUsageRaw {
                input_tokens: 100,
                ..TokenUsageRaw::default()
            },
            reasoning_tokens: 0,
        };
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(r#"{"sample":{"input_cost_per_token":0,"output_cost_per_token":0},"vendor/sample":{"input_cost_per_token":1,"output_cost_per_token":1}}"#);
        assert_eq!(
            price_droid(&entry, PriceTable::Calculate(&pricing)),
            (0.0, None)
        );
    }

    use super::*;

    #[test]
    fn diagnoses_settings_without_a_usable_timestamp_or_file_mtime() {
        let fixture = ccusage_test_support::fs_fixture!({
            "session.settings.json": r#"{"tokenUsage":{"inputTokens":1}}"#
        });
        let path = fixture.path("session.settings.json");
        std::fs::File::open(&path)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(std::time::UNIX_EPOCH - std::time::Duration::from_secs(1)),
            )
            .unwrap();
        let error = load_settings_file(&path)
            .err()
            .expect("missing timestamp must be diagnosed");
        let diagnostics = ccusage_core::Diagnostics::new("droid");
        error.record(&path, &diagnostics);
        assert_eq!(
            diagnostics.skipped(ccusage_core::SkipKind::InvalidTimestamp),
            1
        );
    }

    fn deepseek_pricing() -> ccusage_core::PricingMap {
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(
            r#"{
                "deepseek-v4-flash": {
                    "input_cost_per_token": 0.00000014,
                    "output_cost_per_token": 0.00000028
                }
            }"#,
        );
        pricing
    }

    #[test]
    fn does_not_use_display_timestamp_for_droid_pricing() {
        let timestamp = parse_ts_timestamp("2026-08-17T01:00:00Z").unwrap();
        let entry = DroidEntry {
            timestamp,
            timestamp_text: format_rfc3339_millis(timestamp),
            pricing_timestamp: None,
            session_id: "session".to_string(),
            model: "deepseek-v4-flash".to_string(),
            provider: "deepseek".to_string(),
            provider_locked: false,
            usage: TokenUsageRaw {
                input_tokens: 1_000_000,
                ..TokenUsageRaw::default()
            },
            reasoning_tokens: 0,
        };

        assert_eq!(
            price_droid(&entry, PriceTable::Auto(&deepseek_pricing())).0,
            0.14
        );
    }

    #[test]
    fn preserves_provider_lock_timestamp_for_pricing() {
        let fixture = ccusage_test_support::fs_fixture!({
            "session.settings.json": r#"{
                "providerLockTimestamp": "2026-08-17T01:00:00Z",
                "tokenUsage": {"inputTokens": 1}
            }"#
        });

        let entry = load_settings_file(&fixture.path("session.settings.json"))
            .unwrap()
            .unwrap();

        assert_eq!(
            entry.pricing_timestamp,
            Some(parse_ts_timestamp("2026-08-17T01:00:00Z").unwrap())
        );
    }

    #[test]
    fn leaves_pricing_timestamp_empty_when_only_file_metadata_is_available() {
        let fixture = ccusage_test_support::fs_fixture!({
            "session.settings.json": r#"{
                "tokenUsage": {"inputTokens": 1}
            }"#
        });

        let entry = load_settings_file(&fixture.path("session.settings.json"))
            .unwrap()
            .unwrap();

        assert_eq!(entry.pricing_timestamp, None);
    }
}

#[cfg(test)]
mod pricing_identity_tests {
    use super::*;

    #[test]
    fn locked_glm_preserves_provider_rates_with_a_bare_base_override() {
        ccusage_test_support::install_pricing_snapshots();
        let model = "glm-4.7".to_owned();
        let rates = ccusage_core::cli::PricingOverride {
            input_cost_per_token: Some(0.00005),
            output_cost_per_token: Some(0.00005),
            ..ccusage_core::cli::PricingOverride::default()
        };
        let pricing =
            ccusage_core::PricingMap::load_embedded().with_user_overrides([(&model, &rates)]);
        let fixture = ccusage_test_support::fs_fixture!({
            "input.settings.json": r#"{"model":"glm-4.7","providerLock":"zai","providerLockTimestamp":"2026-11-17T10:00:00Z","tokenUsage":{"inputTokens":150000}}"#,
            "output.settings.json": r#"{"model":"glm-4.7","providerLock":"zai","providerLockTimestamp":"2026-11-17T10:00:00Z","tokenUsage":{"outputTokens":150000}}"#
        });
        for (name, expected) in [
            ("input.settings.json", 0.09),
            ("output.settings.json", 0.33),
        ] {
            let entry = load_settings_file(&fixture.path(name)).unwrap().unwrap();
            let (cost, missing) = price_droid(&entry, PriceTable::Calculate(&pricing));
            assert!((cost - expected).abs() < 1e-12, "{name}: {cost}");
            assert_eq!(missing, None);
        }
    }

    #[test]
    fn locked_model_prefers_a_primary_provider_rate() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        for (model, provider, input_tokens, expected) in [
            ("glm-4.7", "zai", 150_000, 0.09),
            ("gemini-2.5-flash", "google", 1_000, 0.0003),
        ] {
            let fixture = ccusage_test_support::fs_fixture!({
                "s.settings.json": serde_json::json!({"model": model, "providerLock": provider,
                    "providerLockTimestamp": "2026-11-17T10:00:00Z", "tokenUsage": {"inputTokens": input_tokens}
                }).to_string()
            });
            let entry = load_settings_file(&fixture.path("s.settings.json"))
                .unwrap()
                .unwrap();
            let (cost, missing) = price_droid(&entry, PriceTable::Calculate(&pricing));
            assert!((cost - expected).abs() < 1e-12, "{model}: {cost}");
            assert_eq!(missing, None);
        }
    }

    #[test]
    fn adapter_pricing_identity() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        for (raw, normalized, input, output) in [
            ("gpt-5.1", "gpt-5-1", 1.25, 10.0),
            ("gpt-5.1-codex", "gpt-5-1-codex", 1.25, 10.0),
            ("gpt-5.2", "gpt-5-2", 1.75, 14.0),
            ("gpt-5.4", "gpt-5-4", 2.5, 15.0),
            ("gpt-5.5", "gpt-5-5", 5.0, 30.0),
            ("gpt-5.6", "gpt-5-6", 4.0, 20.0),
            ("gpt-4.1", "gpt-4-1", 2.0, 8.0),
            ("gemini-2.5-flash", "gemini-2-5-flash", 0.3, 2.5),
            ("gemini-3.5-flash", "gemini-3-5-flash", 1.5, 9.0),
            ("glm-4.7", "glm-4-7", 0.6, 2.2),
            ("o1", "o1", 15.0, 60.0),
            ("deepseek-v3.2", "deepseek-v3-2", 0.28, 0.42),
            (
                "deepseek-v3-2-251201",
                "deepseek-v3-2-251201",
                0.28985507,
                0.43478261,
            ),
        ] {
            let fixture = ccusage_test_support::fs_fixture!({
                "s.settings.json": serde_json::json!({"model":raw,"providerLockTimestamp":"2026-11-17T10:00:00Z","tokenUsage":{"inputTokens":1000,"outputTokens":1000}}).to_string()
            });
            let entry = load_settings_file(&fixture.path("s.settings.json"))
                .unwrap()
                .unwrap();
            assert_eq!(entry.model, normalized);
            let (cost, missing) = price_droid(&entry, PriceTable::Calculate(&pricing));
            assert!(
                (cost - (input + output) / 1000.0).abs() < 1e-12,
                "{raw}: {cost}"
            );
            assert_eq!(missing, None);
        }
    }
}
