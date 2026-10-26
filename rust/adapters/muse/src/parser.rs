use std::{path::Path, sync::Arc};

use ccusage_core::{
    LoadedEntry, PricingMap, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    calculate_cost_for_usage_at, cli::CostMode, format_date_tz,
    missing_pricing_model_for_candidates,
};
use jiff::tz::TimeZone as JiffTimeZone;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MuseUsageEntry {
    timestamp: TimestampMs,
    model: Option<String>,
    provider: Option<String>,
    project_path: Option<String>,
    pub(super) record_id: Option<String>,
    pub(super) stream_id: Option<String>,
    pub(super) line_index: usize,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MuseMetadata {
    workspace_root: Option<String>,
    model_id: Option<String>,
    provider_id: Option<String>,
}

#[cfg(test)]
fn parse_model_completed(line: &str) -> Option<MuseUsageEntry> {
    let value = serde_json::from_str::<Value>(line).ok()?;
    parse_model_completed_value(&value, 0)
}

pub(super) fn parse_model_completed_value(
    value: &Value,
    line_index: usize,
) -> Option<MuseUsageEntry> {
    if value.get("payload_type")?.as_str()? != "runtime.session" {
        return None;
    }

    let event = value.get("payload")?.get("event")?;
    if event.get("kind")?.as_str()? != "model_completed" {
        return None;
    }

    let recorded_at = value.get("recorded_at").and_then(parse_i64)?;
    let timestamp = TimestampMs::from_millis(recorded_at.div_euclid(1_000));
    let usage = event.get("usage")?;
    let gross_input_tokens = usage.get("input_tokens").and_then(parse_u64).unwrap_or(0);
    let cache_read_tokens = usage
        .get("cache_read_tokens")
        .and_then(parse_u64)
        .or_else(|| usage.get("cached_tokens").and_then(parse_u64))
        .unwrap_or(0)
        .min(gross_input_tokens);
    let input_tokens = gross_input_tokens.saturating_sub(cache_read_tokens);
    let output_tokens = usage.get("output_tokens").and_then(parse_u64).unwrap_or(0);
    let cache_write_tokens = usage
        .get("cache_write_tokens")
        .and_then(parse_u64)
        .unwrap_or(0);

    if input_tokens == 0 && output_tokens == 0 && cache_read_tokens == 0 && cache_write_tokens == 0
    {
        return None;
    }

    Some(MuseUsageEntry {
        timestamp,
        model: event.get("model").and_then(non_empty_string),
        provider: event.get("provider").and_then(non_empty_string),
        project_path: None,
        record_id: value.get("id").and_then(value_to_string),
        stream_id: value
            .get("stream")
            .and_then(|stream| stream.get("id"))
            .and_then(value_to_string),
        line_index,
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
    })
}

pub(super) fn parse_metadata_record(value: &Value) -> Option<MuseMetadata> {
    if value.get("payload_type")?.as_str()? != "runtime.session.metadata" {
        return None;
    }

    let record = value.get("payload")?.get("record")?;
    Some(MuseMetadata {
        workspace_root: record.get("workspace_root").and_then(non_empty_string),
        model_id: record.get("model_id").and_then(non_empty_string),
        provider_id: record
            .get("provider_id")
            .and_then(non_empty_string)
            .filter(|provider| {
                !provider.eq_ignore_ascii_case("unknown") && !provider.eq_ignore_ascii_case("muse")
            }),
    })
}

impl MuseMetadata {
    pub(super) fn apply_to(&self, entry: &mut MuseUsageEntry) {
        if entry.project_path.is_none() {
            entry.project_path.clone_from(&self.workspace_root);
        }
        if entry.model.is_none() {
            entry.model.clone_from(&self.model_id);
        }
        if entry.provider.is_none()
            && self
                .model_id
                .as_deref()
                .is_some_and(|metadata_model| entry.model.as_deref() == Some(metadata_model))
        {
            entry.provider.clone_from(&self.provider_id);
        }
    }
}

fn non_empty_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn value_to_string(value: &Value) -> Option<String> {
    non_empty_string(value).or_else(|| value.as_i64().map(|value| value.to_string()))
}

fn parse_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|value| u64::try_from(value).ok()))
        .or_else(|| value.as_str()?.trim().parse().ok())
}

fn parse_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .or_else(|| value.as_str()?.trim().parse().ok())
}

pub(super) fn muse_entry_to_loaded(
    entry: MuseUsageEntry,
    session_id: &str,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: &PricingMap,
) -> LoadedEntry {
    let usage = TokenUsageRaw {
        input_tokens: entry.input_tokens,
        output_tokens: entry.output_tokens,
        cache_creation_input_tokens: entry.cache_write_tokens,
        cache_read_input_tokens: entry.cache_read_tokens,
        speed: None,
        cache_creation: None,
    };
    let model = entry.model.clone().unwrap_or_else(|| "unknown".to_owned());
    let project_path = entry
        .project_path
        .clone()
        .unwrap_or_else(|| "Muse Code".to_owned());
    let project = Path::new(&project_path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Muse Code")
        .to_owned();
    let timestamp_text = crate::format_rfc3339_millis(entry.timestamp);
    let cost = calculate_muse_cost(&entry, mode, pricing, usage);
    let missing_pricing_model = missing_muse_pricing(&entry, mode, pricing, usage);

    LoadedEntry {
        data: UsageEntry {
            session_id: Some(session_id.to_owned()),
            timestamp: timestamp_text,
            version: None,
            message: UsageMessage {
                usage,
                model: Some(model.clone()),
                id: entry.record_id,
            },
            cost_usd: None,
            request_id: None,
            is_api_error_message: None,
            is_sidechain: None,
        },
        timestamp: entry.timestamp,
        date: format_date_tz(entry.timestamp, tz),
        project: Arc::from(project),
        session_id: Arc::from(session_id),
        project_path: Arc::from(project_path),
        cost,
        extra_total_tokens: 0,
        credits: None,
        message_count: None,
        model: Some(model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        stored_cost: None,
    }
}

fn calculate_muse_cost(
    entry: &MuseUsageEntry,
    mode: CostMode,
    pricing: &PricingMap,
    usage: TokenUsageRaw,
) -> f64 {
    let candidates = model_candidates(entry);
    let Some(candidate) = candidates
        .into_iter()
        .find(|candidate| pricing.find(candidate).is_some())
    else {
        return 0.0;
    };
    calculate_cost_for_usage_at(
        Some(&candidate),
        usage,
        None,
        Some(entry.timestamp),
        mode,
        Some(pricing),
    )
}

fn missing_muse_pricing(
    entry: &MuseUsageEntry,
    mode: CostMode,
    pricing: &PricingMap,
    usage: TokenUsageRaw,
) -> Option<String> {
    if mode == CostMode::Display {
        return None;
    }
    let model = entry.model.as_deref().unwrap_or("unknown");
    missing_pricing_model_for_candidates(
        model,
        model_candidates(entry),
        crate::total_usage_tokens(usage),
        Some(pricing),
    )
}

fn model_candidates(entry: &MuseUsageEntry) -> Vec<String> {
    let Some(model) = entry.model.as_deref().filter(|model| !model.is_empty()) else {
        return Vec::new();
    };
    if model.contains('/') {
        return vec![model.to_owned()];
    }

    match entry.provider.as_deref().map(str::trim) {
        Some(provider) if !provider.is_empty() && !provider.eq_ignore_ascii_case("muse") => {
            if provider.eq_ignore_ascii_case("meta") && model.starts_with("muse-spark-") {
                vec![model.to_owned(), format!("meta/{model}")]
            } else {
                vec![format!("{provider}/{model}")]
            }
        }
        _ if model.starts_with("muse-spark-") => {
            vec![model.to_owned(), format!("meta/{model}")]
        }
        _ => vec![model.to_owned()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_empirical_muse_model_completed_record() {
        let line = include_str!("../fixtures/issue-1694-model-completed.jsonl");
        let entry = parse_model_completed(line.trim())
            .expect("the empirical model_completed record should produce usage");

        assert_eq!(entry.timestamp.as_millis(), 1_788_797_051_685);
        assert_eq!(entry.model.as_deref(), Some("muse-spark-1.3-contributor"));
        assert_eq!(entry.input_tokens, 610);
        assert_eq!(entry.output_tokens, 20);
        assert_eq!(entry.cache_read_tokens, 27_121);
        assert_eq!(entry.cache_write_tokens, 0);
    }

    #[test]
    fn prefers_cache_read_tokens_and_clamps_to_gross_input() {
        let line = r#"{"recorded_at":0,"payload_type":"runtime.session","payload":{"event":{"kind":"model_completed","model":"muse-spark-1.2","usage":{"input_tokens":10,"output_tokens":4,"cache_read_tokens":50,"cached_tokens":3,"cache_write_tokens":2,"reasoning_tokens":100}}}}"#;
        let entry = parse_model_completed(line).unwrap();

        assert_eq!(entry.input_tokens, 0);
        assert_eq!(entry.cache_read_tokens, 10);
        assert_eq!(entry.output_tokens, 4);
        assert_eq!(entry.cache_write_tokens, 2);
    }

    #[test]
    fn falls_back_to_cached_alias_and_does_not_add_reasoning() {
        let line = r#"{"recorded_at":0,"payload_type":"runtime.session","payload":{"event":{"kind":"model_completed","model":"muse-spark-1.2","usage":{"input_tokens":10,"output_tokens":4,"cached_tokens":7,"reasoning_tokens":100}}}}"#;
        let entry = parse_model_completed(line).unwrap();

        assert_eq!(entry.input_tokens, 3);
        assert_eq!(entry.cache_read_tokens, 7);
        assert_eq!(entry.output_tokens, 4);
    }

    #[test]
    fn reads_only_the_verified_metadata_record_fields() {
        let line = include_str!("../fixtures/metadata-timeline-synthetic.jsonl")
            .lines()
            .nth(1)
            .unwrap();
        let value = serde_json::from_str::<Value>(line).unwrap();
        let metadata = parse_metadata_record(&value).unwrap();

        assert_eq!(
            metadata.workspace_root.as_deref(),
            Some("/workspace/initial")
        );
        assert_eq!(metadata.model_id.as_deref(), Some("muse-spark-1.2"));
        assert_eq!(metadata.provider_id.as_deref(), Some("openrouter"));

        let decoy = serde_json::json!({
            "payload_type": "runtime.session.metadata",
            "payload": { "context": {
                "workspace_root": "/unrelated",
                "model_id": "unrelated-model",
                "provider_id": "unrelated-provider"
            } }
        });
        assert!(parse_metadata_record(&decoy).is_none());
    }

    #[test]
    fn metadata_provider_applies_only_to_its_model_and_ignores_sentinels() {
        let mut primary_model = parse_model_completed(
            r#"{"recorded_at":0,"payload_type":"runtime.session","payload":{"event":{"kind":"model_completed","model":"muse-spark-1.2","usage":{"input_tokens":1}}}}"#,
        )
        .unwrap();
        let contributor_metadata = parse_metadata_record(&serde_json::json!({
            "payload_type": "runtime.session.metadata",
            "payload": { "record": {
                "model_id": "muse-spark-1.2-contributor",
                "provider_id": "meta"
            } }
        }))
        .unwrap();
        contributor_metadata.apply_to(&mut primary_model);
        assert_eq!(primary_model.model.as_deref(), Some("muse-spark-1.2"));
        assert_eq!(primary_model.provider, None);
        assert_eq!(
            model_candidates(&primary_model),
            ["muse-spark-1.2", "meta/muse-spark-1.2"]
        );

        let mut missing_model = parse_model_completed(
            r#"{"recorded_at":0,"payload_type":"runtime.session","payload":{"event":{"kind":"model_completed","usage":{"input_tokens":1}}}}"#,
        )
        .unwrap();
        let openrouter_metadata = parse_metadata_record(&serde_json::json!({
            "payload_type": "runtime.session.metadata",
            "payload": { "record": {
                "model_id": "muse-spark-1.2",
                "provider_id": "openrouter"
            } }
        }))
        .unwrap();
        openrouter_metadata.apply_to(&mut missing_model);
        assert_eq!(missing_model.model.as_deref(), Some("muse-spark-1.2"));
        assert_eq!(
            model_candidates(&missing_model),
            ["openrouter/muse-spark-1.2"]
        );

        for sentinel in ["unknown", "muse"] {
            let mut entry = parse_model_completed(
                r#"{"recorded_at":0,"payload_type":"runtime.session","payload":{"event":{"kind":"model_completed","usage":{"input_tokens":1}}}}"#,
            )
            .unwrap();
            let metadata = parse_metadata_record(&serde_json::json!({
                "payload_type": "runtime.session.metadata",
                "payload": { "record": {
                    "model_id": "muse-spark-1.2",
                    "provider_id": sentinel
                } }
            }))
            .unwrap();
            metadata.apply_to(&mut entry);
            assert_eq!(entry.provider, None);
        }
    }

    #[test]
    fn skips_invalid_envelopes_timestamps_and_zero_usage() {
        assert!(parse_model_completed("not json").is_none());
        assert!(parse_model_completed(
            r#"{"recorded_at":"bad","payload_type":"runtime.session","payload":{"event":{"kind":"model_completed","usage":{"input_tokens":1}}}}"#
        )
        .is_none());
        assert!(parse_model_completed(
            r#"{"recorded_at":0,"payload_type":"runtime.other","payload":{"event":{"kind":"model_completed","usage":{"input_tokens":1}}}}"#
        )
        .is_none());
        assert!(parse_model_completed(
            r#"{"recorded_at":0,"payload_type":"runtime.session","payload":{"event":{"kind":"model_started","usage":{"input_tokens":1}}}}"#
        )
        .is_none());
        assert!(parse_model_completed(
            r#"{"recorded_at":0,"payload_type":"runtime.session","payload":{"event":{"kind":"model_completed","usage":{"input_tokens":0,"output_tokens":0,"cache_read_tokens":0,"cache_write_tokens":0}}}}"#
        )
        .is_none());
    }

    #[test]
    fn uses_bare_muse_spark_model_for_embedded_offline_pricing() {
        let pricing = PricingMap::load_embedded();
        let entry = MuseUsageEntry {
            timestamp: TimestampMs::from_millis(1_788_797_051_685),
            model: Some("muse-spark-1.2".to_owned()),
            provider: None,
            project_path: None,
            record_id: None,
            stream_id: None,
            line_index: 0,
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_read_tokens: 1_000_000,
            cache_write_tokens: 0,
        };
        let loaded = muse_entry_to_loaded(entry, "session-a", None, CostMode::Calculate, &pricing);

        assert!((loaded.cost - 5.65).abs() < 0.000_001);
        assert_eq!(loaded.missing_pricing_model, None);
    }

    #[test]
    fn retains_non_meta_provider_and_contributor_model_identity() {
        let entry = MuseUsageEntry {
            timestamp: TimestampMs::UNIX_EPOCH,
            model: Some("muse-spark-1.2-contributor".to_owned()),
            provider: Some("openrouter".to_owned()),
            project_path: None,
            record_id: None,
            stream_id: None,
            line_index: 0,
            input_tokens: 1,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };

        assert_eq!(
            model_candidates(&entry),
            ["openrouter/muse-spark-1.2-contributor"]
        );
    }

    #[test]
    fn leaves_missing_models_unknown_without_pricing_fallback() {
        let entry = MuseUsageEntry {
            timestamp: TimestampMs::UNIX_EPOCH,
            model: None,
            provider: Some("meta".to_owned()),
            project_path: None,
            record_id: None,
            stream_id: None,
            line_index: 0,
            input_tokens: 10,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };
        let loaded = muse_entry_to_loaded(
            entry,
            "session-a",
            None,
            CostMode::Calculate,
            &PricingMap::load_embedded(),
        );

        assert_eq!(loaded.model.as_deref(), Some("unknown"));
        assert_eq!(loaded.cost, 0.0);
        assert_eq!(loaded.missing_pricing_model.as_deref(), Some("unknown"));
    }
}
