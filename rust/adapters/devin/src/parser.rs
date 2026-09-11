use std::{fs, io, path::Path};

use serde_json::Value;

use crate::{
    PricingMap, Result, TokenUsageRaw, calculate_cost_for_usage_at, cli::CostMode,
    format_rfc3339_millis, json_value_u64, missing_pricing_model_for_candidates,
    parse_ts_timestamp,
};

#[derive(Clone)]
pub(super) struct DevinStep {
    pub(super) timestamp: crate::TimestampMs,
    pub(super) timestamp_text: String,
    pub(super) session_id: String,
    pub(super) step_key: String,
    pub(super) model: String,
    pub(super) version: Option<String>,
    pub(super) usage: TokenUsageRaw,
}

/// One transcript file is one session; every `agent` step with `metrics` is one
/// model call, so a session can span report periods and switch models.
pub(super) fn load_transcript_file(path: &Path) -> Result<Vec<DevinStep>> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let value = serde_json::from_str::<Value>(&content).map_err(|error| {
        crate::cli_error(format!(
            "failed to parse Devin transcript {}: {error}",
            path.display()
        ))
    })?;
    let Some(transcript) = value.as_object() else {
        return Ok(Vec::new());
    };

    let fallback_session_id = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix(".json"))
        .unwrap_or("unknown");
    let session_id =
        string_field(transcript, "session_id").unwrap_or_else(|| fallback_session_id.to_string());
    let agent = transcript.get("agent").and_then(Value::as_object);
    let version = agent.and_then(|agent| string_field(agent, "version"));
    let fallback_model = agent
        .and_then(|agent| string_field(agent, "model_name"))
        .map(|model| normalize_devin_model_name(&model));

    let mut steps = Vec::new();
    for step in transcript
        .get("steps")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
    {
        // Older exports name the model-call step "assistant"; "agent" is current.
        if !matches!(
            step.get("source").and_then(Value::as_str),
            Some("agent") | Some("assistant")
        ) {
            continue;
        }
        let metadata = step.get("metadata").and_then(Value::as_object);
        // v1.7 puts metrics on the step; v1.4 nests them under metadata.
        let Some(usage) = step
            .get("metrics")
            .and_then(Value::as_object)
            .map(step_usage)
            .or_else(|| {
                metadata
                    .and_then(|metadata| metadata.get("metrics"))
                    .and_then(Value::as_object)
                    .map(legacy_step_usage)
            })
        else {
            continue;
        };
        let Some(timestamp) = step
            .get("timestamp")
            .and_then(Value::as_str)
            .or_else(|| {
                metadata
                    .and_then(|metadata| metadata.get("created_at"))
                    .and_then(Value::as_str)
            })
            .and_then(parse_devin_timestamp)
        else {
            continue;
        };
        if crate::total_usage_tokens(usage) == 0 {
            continue;
        }
        let model = step
            .get("extra")
            .and_then(Value::as_object)
            .and_then(|extra| string_field(extra, "generation_model"))
            .or_else(|| metadata.and_then(|metadata| string_field(metadata, "generation_model")))
            .map(|model| normalize_devin_model_name(&model))
            .or_else(|| {
                step.get("model_name")
                    .and_then(Value::as_str)
                    .map(normalize_devin_model_name)
            })
            .or_else(|| fallback_model.clone())
            .unwrap_or_else(|| "unknown".to_string());
        // step_id can be missing or non-string; the fallback ordinal is scoped
        // to the file stem so transcripts sharing a session_id do not collide.
        let step_key = step
            .get("step_id")
            .map(|id| {
                id.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| id.to_string())
            })
            .unwrap_or_else(|| format!("{fallback_session_id}:{}", steps.len()));
        steps.push(DevinStep {
            timestamp,
            timestamp_text: format_rfc3339_millis(timestamp),
            session_id: session_id.clone(),
            step_key,
            model,
            version: version.clone(),
            usage,
        });
    }
    Ok(steps)
}

/// `prompt_tokens` is the whole request context: the uncached remainder plus
/// cache reads plus cache creation. Cost and total-token math expect the
/// Claude shape, where `input_tokens` excludes both cache buckets.
fn step_usage(metrics: &serde_json::Map<String, Value>) -> TokenUsageRaw {
    let prompt_tokens = json_value_u64(metrics.get("prompt_tokens"));
    let cache_read = json_value_u64(metrics.get("cached_tokens"));
    let cache_creation = metrics
        .get("extra")
        .and_then(Value::as_object)
        .map_or(0, |extra| {
            json_value_u64(extra.get("cache_creation_input_tokens"))
        });
    TokenUsageRaw {
        input_tokens: prompt_tokens
            .saturating_sub(cache_read)
            .saturating_sub(cache_creation),
        output_tokens: json_value_u64(metrics.get("completion_tokens")),
        cache_creation_input_tokens: cache_creation,
        cache_read_input_tokens: cache_read,
        speed: None,
        cache_creation: None,
    }
}

/// v1.4 steps carry `metadata.metrics` where `input_tokens` already excludes
/// the cache buckets, so the fields map directly onto the shared shape.
fn legacy_step_usage(metrics: &serde_json::Map<String, Value>) -> TokenUsageRaw {
    TokenUsageRaw {
        input_tokens: json_value_u64(metrics.get("input_tokens")),
        output_tokens: json_value_u64(metrics.get("output_tokens")),
        cache_creation_input_tokens: json_value_u64(metrics.get("cache_creation_tokens")),
        cache_read_input_tokens: json_value_u64(metrics.get("cache_read_tokens")),
        speed: None,
        cache_creation: None,
    }
}

/// The shared parser takes exactly three fraction digits, while transcripts
/// carry microseconds ("2026-09-11T03:21:00.807570+00:00"), so the fraction is
/// truncated (or padded) to millis before falling back to it.
fn parse_devin_timestamp(value: &str) -> Option<crate::TimestampMs> {
    if let Some(timestamp) = parse_ts_timestamp(value) {
        return Some(timestamp);
    }
    let bytes = value.as_bytes();
    if bytes.len() <= 20 || bytes.get(19) != Some(&b'.') {
        return None;
    }
    let fraction_end = bytes[20..]
        .iter()
        .position(|byte| !byte.is_ascii_digit())
        .map_or(bytes.len(), |index| 20 + index);
    if fraction_end == 20 {
        return None;
    }
    let fraction = &value[20..fraction_end];
    let normalized = format!(
        "{}.{:0<3}{}",
        &value[..19],
        &fraction[..fraction.len().min(3)],
        &value[fraction_end..]
    );
    parse_ts_timestamp(&normalized)
}

fn string_field(record: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    let value = record.get(key)?.as_str()?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// Display names such as "SWE-2 High" become the API-style ids LiteLLM indexes
/// ("swe-2-high"). Dots and slashes survive because the pricing table keeps
/// them ("cognition/swe-1.7").
pub fn normalize_devin_model_name(model: &str) -> String {
    let mut normalized = String::new();
    let mut previous_dash = false;
    for ch in model.trim().chars().flat_map(char::to_lowercase) {
        let next = if ch.is_ascii_alphanumeric() || ch == '.' || ch == '/' {
            ch
        } else {
            '-'
        };
        if next == '-' {
            if previous_dash || normalized.is_empty() {
                continue;
            }
            previous_dash = true;
        } else {
            previous_dash = false;
        }
        normalized.push(next);
    }
    normalized.trim_end_matches('-').to_string()
}

fn devin_model_candidates(step: &DevinStep) -> Vec<String> {
    let mut candidates = vec![step.model.clone()];
    let prefixed = format!("cognition/{}", step.model);
    if !candidates.contains(&prefixed) {
        candidates.push(prefixed);
    }
    candidates
}

pub(super) fn calculate_devin_cost(step: &DevinStep, pricing: &PricingMap) -> f64 {
    for candidate in devin_model_candidates(step) {
        let cost = calculate_cost_for_usage_at(
            Some(&candidate),
            step.usage,
            None,
            Some(step.timestamp),
            CostMode::Calculate,
            Some(pricing),
        );
        if cost > 0.0 {
            return cost;
        }
    }
    0.0
}

pub(super) fn missing_devin_pricing(step: &DevinStep, pricing: &PricingMap) -> Option<String> {
    missing_pricing_model_for_candidates(
        &step.model,
        devin_model_candidates(step),
        crate::total_usage_tokens(step.usage),
        Some(pricing),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pricing() -> PricingMap {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{
                "cognition/swe-2-high": {
                    "input_cost_per_token": 0.0000005,
                    "output_cost_per_token": 0.0000025,
                    "cache_read_input_token_cost": 0.0000002,
                    "cache_creation_input_token_cost": 0.000000625
                }
            }"#,
        );
        pricing
    }

    fn make_step(model: &str, usage: TokenUsageRaw) -> DevinStep {
        DevinStep {
            timestamp: parse_ts_timestamp("2026-09-01T00:00:00.000Z").unwrap(),
            timestamp_text: "2026-09-01T00:00:00.000Z".to_string(),
            session_id: "session".to_string(),
            step_key: "1".to_string(),
            model: model.to_string(),
            version: None,
            usage,
        }
    }

    #[test]
    fn subtracts_cache_buckets_from_prompt_tokens() {
        let usage = step_usage(
            &serde_json::from_str::<Value>(
                r#"{"prompt_tokens": 26828, "completion_tokens": 109, "cached_tokens": 9317, "extra": {"cache_creation_input_tokens": 17508}}"#,
            )
            .unwrap()
            .as_object()
            .unwrap()
            .clone(),
        );

        assert_eq!(usage.input_tokens, 3);
        assert_eq!(usage.output_tokens, 109);
        assert_eq!(usage.cache_read_input_tokens, 9317);
        assert_eq!(usage.cache_creation_input_tokens, 17508);
    }

    #[test]
    fn saturates_when_cache_exceeds_prompt_tokens() {
        let usage = step_usage(
            &serde_json::from_str::<Value>(r#"{"prompt_tokens": 10, "cached_tokens": 50}"#)
                .unwrap()
                .as_object()
                .unwrap()
                .clone(),
        );

        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.cache_read_input_tokens, 50);
    }

    #[test]
    fn prices_through_the_cognition_provider_prefix() {
        let usage = TokenUsageRaw {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 1_000_000,
            speed: None,
            cache_creation: None,
        };

        let cost = calculate_devin_cost(&make_step("swe-2-high", usage), &pricing());

        assert!((cost - 3.2).abs() < 1e-9, "cost was {cost}");
    }

    #[test]
    fn flags_models_without_any_pricing_candidate() {
        let step = make_step(
            "swe-unknown",
            TokenUsageRaw {
                input_tokens: 1,
                ..TokenUsageRaw::default()
            },
        );

        assert_eq!(
            missing_devin_pricing(&step, &pricing()).as_deref(),
            Some("swe-unknown")
        );

        let priced = make_step(
            "swe-2-high",
            TokenUsageRaw {
                input_tokens: 1,
                ..TokenUsageRaw::default()
            },
        );
        assert!(missing_devin_pricing(&priced, &pricing()).is_none());
    }

    #[test]
    fn parses_transcript_timestamp_precisions() {
        let millis = parse_devin_timestamp("2026-09-11T03:21:00.807Z");
        let micros = parse_devin_timestamp("2026-09-11T03:21:00.807570+00:00");
        let seconds = parse_devin_timestamp("2026-09-11T03:21:00+00:00");

        assert!(millis.is_some() && micros.is_some() && seconds.is_some());
        assert_eq!(micros.unwrap().as_millis(), millis.unwrap().as_millis());
        assert_eq!(
            seconds.unwrap().as_millis() + 807,
            millis.unwrap().as_millis()
        );
        assert!(parse_devin_timestamp("not-a-date").is_none());
    }

    #[test]
    fn normalizes_display_names_to_api_ids() {
        assert_eq!(normalize_devin_model_name("SWE-2 High"), "swe-2-high");
        assert_eq!(normalize_devin_model_name("SWE-1.7"), "swe-1.7");
        assert_eq!(
            normalize_devin_model_name("cognition/SWE-2-High"),
            "cognition/swe-2-high"
        );
        assert_eq!(
            normalize_devin_model_name("  Claude Sonnet 4 "),
            "claude-sonnet-4"
        );
    }
}
