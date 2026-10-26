use std::{path::Path, sync::Arc};

use ccusage_core::{
    LoadedEntry, PricingMap, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    calculate_cost_from_pricing, cli::CostMode, format_date_tz,
};
use jiff::{Timestamp as JiffTimestamp, tz::TimeZone as JiffTimeZone};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MuseUsageEntry {
    timestamp: TimestampMs,
    recorded_at_micros: i64,
    model: String,
    provider: Option<String>,
    event_provider_present: bool,
    project_path: Option<String>,
    pub(super) record_id: Option<String>,
    pub(super) stream_id: Option<String>,
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

impl MuseUsageEntry {
    pub(super) fn is_meta_routed(&self) -> bool {
        self.provider
            .as_deref()
            .is_some_and(|provider| provider.eq_ignore_ascii_case("meta"))
    }

    pub(super) fn has_same_required_payload(&self, other: &Self) -> bool {
        self.recorded_at_micros == other.recorded_at_micros
            && self.model == other.model
            && self.input_tokens == other.input_tokens
            && self.output_tokens == other.output_tokens
            && self.cache_read_tokens == other.cache_read_tokens
            && self.cache_write_tokens == other.cache_write_tokens
            && self.provider == other.provider
    }
}

#[cfg(test)]
fn parse_model_completed(line: &str) -> Option<MuseUsageEntry> {
    let value = serde_json::from_str::<Value>(line).ok()?;
    parse_model_completed_value(&value)
}

pub(super) fn parse_model_completed_value(value: &Value) -> Option<MuseUsageEntry> {
    if value.get("payload_type")?.as_str()? != "runtime.session" {
        return None;
    }

    let event = value.get("payload")?.get("event")?;
    if event.get("kind")?.as_str()? != "model_completed" {
        return None;
    }

    let recorded_at_micros = value.get("recorded_at").and_then(parse_i64)?;
    if recorded_at_micros <= 0 {
        return None;
    }
    JiffTimestamp::from_microsecond(recorded_at_micros).ok()?;
    let timestamp = TimestampMs::from_millis(recorded_at_micros.div_euclid(1_000));
    let model = event.get("model").and_then(non_empty_model)?;
    if !model.starts_with("muse-spark-") {
        return None;
    }
    let usage = event.get("usage")?;
    let gross_input_tokens = usage.get("input_tokens").and_then(parse_u64)?;
    let output_tokens = usage.get("output_tokens").and_then(parse_u64)?;
    let cache_read_tokens = usage.get("cache_read_tokens").and_then(parse_u64)?;
    let cache_write_tokens = usage.get("cache_write_tokens").and_then(parse_u64)?;
    if cache_read_tokens > gross_input_tokens || cache_write_tokens != 0 {
        return None;
    }

    Some(MuseUsageEntry {
        timestamp,
        recorded_at_micros,
        model,
        provider: event.get("provider").and_then(non_empty_string),
        event_provider_present: event.get("provider").is_some(),
        project_path: None,
        record_id: value.get("id").and_then(opaque_id),
        stream_id: value
            .get("stream")
            .and_then(|stream| stream.get("id"))
            .and_then(opaque_id),
        input_tokens: gross_input_tokens - cache_read_tokens,
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
        model_id: record.get("model_id").and_then(non_empty_model),
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
        if !entry.event_provider_present
            && entry.provider.is_none()
            && self.model_id.as_deref() == Some(entry.model.as_str())
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

fn non_empty_model(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
}

fn opaque_id(value: &Value) -> Option<String> {
    let id = value.as_str()?;
    (!id.trim().is_empty()).then(|| id.to_owned())
}

fn parse_u64(value: &Value) -> Option<u64> {
    value.as_u64()
}

fn parse_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
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
    let model = entry.model.clone();
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
    if !entry.is_meta_routed() {
        return 0.0;
    }
    let Some(pricing) = model_candidates(entry)
        .into_iter()
        .find_map(|candidate| pricing.find_exact_with_fallback(&candidate))
    else {
        return 0.0;
    };
    if mode == CostMode::Display {
        return 0.0;
    }
    calculate_cost_from_pricing(usage, pricing)
}

fn missing_muse_pricing(
    entry: &MuseUsageEntry,
    mode: CostMode,
    pricing: &PricingMap,
    usage: TokenUsageRaw,
) -> Option<String> {
    if mode == CostMode::Display || crate::total_usage_tokens(usage) == 0 || !entry.is_meta_routed()
    {
        return None;
    }
    model_candidates(entry)
        .into_iter()
        .all(|candidate| pricing.find_exact_with_fallback(&candidate).is_none())
        .then(|| entry.model.clone())
}

fn model_candidates(entry: &MuseUsageEntry) -> Vec<String> {
    let model = entry.model.as_str();
    vec![model.to_owned(), format!("meta/{model}")]
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
        assert_eq!(entry.model, "muse-spark-1.3-contributor");
        assert_eq!(entry.input_tokens, 610);
        assert_eq!(entry.output_tokens, 20);
        assert_eq!(entry.cache_read_tokens, 27_121);
        assert_eq!(entry.cache_write_tokens, 0);
        assert_eq!(entry.stream_id.as_deref(), Some("01a07c8d-[redacted]"));
        assert_eq!(entry.record_id, None);
        assert!(!entry.is_meta_routed());
    }

    #[test]
    fn rejects_nonzero_cache_writes_without_a_verified_tariff() {
        let mut record = complete_record();
        record["payload"]["event"]["usage"]["cache_write_tokens"] = serde_json::json!(1);

        assert!(parse_value(&record).is_none());
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
        assert_eq!(metadata.provider_id.as_deref(), Some("meta"));

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
        let mut primary_model = parse_value(&complete_record()).unwrap();
        let contributor_metadata = parse_metadata_record(&serde_json::json!({
            "payload_type": "runtime.session.metadata",
            "payload": { "record": {
                "model_id": "muse-spark-1.2-contributor",
                "provider_id": "meta"
            } }
        }))
        .unwrap();
        contributor_metadata.apply_to(&mut primary_model);
        assert_eq!(primary_model.model, "muse-spark-1.2");
        assert!(!primary_model.is_meta_routed());
        assert_eq!(
            model_candidates(&primary_model),
            ["muse-spark-1.2", "meta/muse-spark-1.2"]
        );

        let matching_metadata = parse_metadata_record(&serde_json::json!({
            "payload_type": "runtime.session.metadata",
            "payload": { "record": {
                "model_id": "muse-spark-1.2",
                "provider_id": "meta"
            } }
        }))
        .unwrap();
        matching_metadata.apply_to(&mut primary_model);
        assert!(primary_model.is_meta_routed());

        let mut differently_spaced_model = parse_value(&complete_record()).unwrap();
        let non_exact_metadata = parse_metadata_record(&serde_json::json!({
            "payload_type": "runtime.session.metadata",
            "payload": { "record": {
                "model_id": "muse-spark-1.2 ",
                "provider_id": "meta"
            } }
        }))
        .unwrap();
        non_exact_metadata.apply_to(&mut differently_spaced_model);
        assert!(!differently_spaced_model.is_meta_routed());

        for sentinel in ["unknown", "muse"] {
            let mut entry = parse_value(&complete_record()).unwrap();
            let metadata = parse_metadata_record(&serde_json::json!({
                "payload_type": "runtime.session.metadata",
                "payload": { "record": {
                    "model_id": "muse-spark-1.2",
                    "provider_id": sentinel
                } }
            }))
            .unwrap();
            metadata.apply_to(&mut entry);
            assert!(!entry.is_meta_routed());
        }

        let mut event_routed = complete_record();
        event_routed["payload"]["event"]["provider"] = serde_json::json!("meta");
        assert!(parse_value(&event_routed).unwrap().is_meta_routed());
    }

    #[test]
    fn explicit_event_provider_is_authoritative_over_metadata() {
        let meta_metadata = parse_metadata_record(&serde_json::json!({
            "payload_type": "runtime.session.metadata",
            "payload": { "record": {
                "model_id": "muse-spark-1.2",
                "provider_id": "meta"
            } }
        }))
        .unwrap();
        let mut explicit_openrouter = complete_record();
        explicit_openrouter["payload"]["event"]["provider"] = serde_json::json!("openrouter");
        let mut openrouter_entry = parse_value(&explicit_openrouter).unwrap();
        meta_metadata.apply_to(&mut openrouter_entry);
        assert!(!openrouter_entry.is_meta_routed());
        assert_eq!(openrouter_entry.provider.as_deref(), Some("openrouter"));

        let openrouter_metadata = parse_metadata_record(&serde_json::json!({
            "payload_type": "runtime.session.metadata",
            "payload": { "record": {
                "model_id": "muse-spark-1.2",
                "provider_id": "openrouter"
            } }
        }))
        .unwrap();
        let mut explicit_meta = complete_record();
        explicit_meta["payload"]["event"]["provider"] = serde_json::json!("meta");
        let mut meta_entry = parse_value(&explicit_meta).unwrap();
        openrouter_metadata.apply_to(&mut meta_entry);
        assert!(meta_entry.is_meta_routed());
        assert_eq!(meta_entry.provider.as_deref(), Some("meta"));
    }

    #[test]
    fn explicit_unknown_event_provider_is_not_filled_from_metadata() {
        let metadata = parse_metadata_record(&serde_json::json!({
            "payload_type": "runtime.session.metadata",
            "payload": { "record": {
                "model_id": "muse-spark-1.2",
                "provider_id": "meta"
            } }
        }))
        .unwrap();
        for provider in ["openrouter", "unknown"] {
            let mut record = complete_record();
            record["payload"]["event"]["provider"] = serde_json::json!(provider);
            let mut entry = parse_value(&record).unwrap();
            metadata.apply_to(&mut entry);
            assert_eq!(entry.provider.as_deref(), Some(provider));
            assert!(!entry.is_meta_routed());
        }
    }

    #[test]
    fn skips_invalid_envelopes_and_timestamps() {
        assert!(parse_model_completed("not json").is_none());
        assert!(parse_model_completed(
            r#"{"recorded_at":"bad","payload_type":"runtime.session","payload":{"event":{"kind":"model_completed","usage":{"input_tokens":1}}}}"#
        )
        .is_none());
        assert!(parse_model_completed(
            r#"{"recorded_at":1000000,"payload_type":"runtime.other","payload":{"event":{"kind":"model_completed","usage":{"input_tokens":1}}}}"#
        )
        .is_none());
        assert!(parse_model_completed(
            r#"{"recorded_at":1000000,"payload_type":"runtime.session","payload":{"event":{"kind":"model_started","usage":{"input_tokens":1}}}}"#
        )
        .is_none());
    }

    #[test]
    fn requires_every_counted_field_on_the_completion_record() {
        for path in [
            &["recorded_at"][..],
            &["payload_type"],
            &["payload", "event", "kind"],
            &["payload", "event", "model"],
            &["payload", "event", "usage", "input_tokens"],
            &["payload", "event", "usage", "output_tokens"],
            &["payload", "event", "usage", "cache_read_tokens"],
            &["payload", "event", "usage", "cache_write_tokens"],
        ] {
            let mut record = complete_record();
            let mut parent = &mut record;
            for key in &path[..path.len() - 1] {
                parent = parent.get_mut(*key).unwrap();
            }
            parent
                .as_object_mut()
                .unwrap()
                .remove(*path.last().unwrap());

            assert!(parse_value(&record).is_none(), "accepted missing {path:?}");
        }
    }

    #[test]
    fn rejects_missing_canonical_cache_fields_even_with_cached_alias() {
        let mut record = complete_record();
        let usage = &mut record["payload"]["event"]["usage"];
        usage.as_object_mut().unwrap().remove("cache_read_tokens");
        usage["cached_tokens"] = serde_json::json!(3);

        assert!(parse_value(&record).is_none());

        let mut record = complete_record();
        record["payload"]["event"]["usage"]
            .as_object_mut()
            .unwrap()
            .remove("cache_write_tokens");

        assert!(parse_value(&record).is_none());
    }

    #[test]
    fn rejects_malformed_token_counts_without_coercion() {
        for field in [
            "input_tokens",
            "output_tokens",
            "cache_read_tokens",
            "cache_write_tokens",
        ] {
            for invalid in [
                serde_json::json!(null),
                serde_json::json!(-1),
                serde_json::json!(1.5),
                serde_json::json!("1"),
            ] {
                let mut record = complete_record();
                record["payload"]["event"]["usage"][field] = invalid.clone();

                assert!(parse_value(&record).is_none(), "accepted {field}={invalid}");
            }
        }
    }

    #[test]
    fn rejects_cache_read_greater_than_gross_input() {
        let mut record = complete_record();
        record["payload"]["event"]["usage"]["input_tokens"] = serde_json::json!(10);
        record["payload"]["event"]["usage"]["cache_read_tokens"] = serde_json::json!(11);

        assert!(parse_value(&record).is_none());
    }

    #[test]
    fn accepts_explicit_zero_counts_with_a_valid_timestamp() {
        let mut record = complete_record();
        for field in [
            "input_tokens",
            "output_tokens",
            "cache_read_tokens",
            "cache_write_tokens",
        ] {
            record["payload"]["event"]["usage"][field] = serde_json::json!(0);
        }

        let entry = parse_value(&record).expect("explicit zero usage is valid");
        assert_eq!(entry.input_tokens, 0);
        assert_eq!(entry.output_tokens, 0);
        assert_eq!(entry.cache_read_tokens, 0);
        assert_eq!(entry.cache_write_tokens, 0);
    }

    #[test]
    fn rejects_zero_negative_malformed_and_out_of_range_timestamps() {
        for invalid in [
            serde_json::json!(0),
            serde_json::json!(-1),
            serde_json::json!("1000000"),
            serde_json::json!(1.5),
            serde_json::json!(i64::MAX),
            serde_json::json!(u64::MAX),
        ] {
            let mut record = complete_record();
            record["recorded_at"] = invalid.clone();

            assert!(
                parse_value(&record).is_none(),
                "accepted recorded_at={invalid}"
            );
        }
    }

    #[test]
    fn requires_model_directly_on_the_completion() {
        let mut record = complete_record();
        record["payload"]["event"]
            .as_object_mut()
            .unwrap()
            .remove("model");

        assert!(parse_value(&record).is_none());
    }

    fn complete_record() -> Value {
        serde_json::json!({
            "recorded_at": 1_000_000,
            "stream": { "id": "stream-a" },
            "id": "record-a",
            "payload_type": "runtime.session",
            "payload": { "event": {
                "kind": "model_completed",
                "model": "muse-spark-1.2",
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 4,
                    "cache_read_tokens": 3,
                    "cache_write_tokens": 0
                }
            } }
        })
    }

    fn parse_value(record: &Value) -> Option<MuseUsageEntry> {
        parse_model_completed(&record.to_string())
    }

    #[test]
    fn uses_exact_official_meta_rates_for_the_five_documented_models() {
        let pricing = PricingMap::load_embedded();
        for (model, expected_cost) in [
            ("muse-spark-1.1", 5.65),
            ("muse-spark-1.2", 5.65),
            ("muse-spark-1.3", 5.65),
            ("muse-spark-1.2-contributor", 0.302),
            ("muse-spark-1.3-contributor", 0.302),
        ] {
            let entry = MuseUsageEntry {
                timestamp: TimestampMs::from_millis(1_788_797_051_685),
                recorded_at_micros: 1_788_797_051_685_241,
                model: model.to_owned(),
                provider: Some("meta".to_owned()),
                event_provider_present: true,
                project_path: None,
                record_id: None,
                stream_id: None,
                input_tokens: 1_000_000,
                output_tokens: 1_000_000,
                cache_read_tokens: 1_000_000,
                cache_write_tokens: 0,
            };
            let loaded =
                muse_entry_to_loaded(entry, "session-a", None, CostMode::Calculate, &pricing);

            assert!(
                (loaded.cost - expected_cost).abs() < 0.000_001,
                "unexpected price for {model}: {}",
                loaded.cost
            );
            assert_eq!(loaded.missing_pricing_model, None);
        }
    }

    #[test]
    fn does_not_use_fuzzy_pricing_for_unmatched_muse_versions() {
        let pricing = PricingMap::load_embedded();
        let model = "muse-spark-1.1-preview";
        assert!(pricing.find(model).is_some());
        assert!(pricing.find_exact_with_fallback(model).is_none());

        let entry = MuseUsageEntry {
            timestamp: TimestampMs::from_millis(1_788_797_051_685),
            recorded_at_micros: 1_788_797_051_685_241,
            model: model.to_owned(),
            provider: Some("meta".to_owned()),
            event_provider_present: true,
            project_path: None,
            record_id: None,
            stream_id: None,
            input_tokens: 1,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };
        let loaded = muse_entry_to_loaded(entry, "session-a", None, CostMode::Calculate, &pricing);

        assert_eq!(loaded.cost, 0.0);
        assert_eq!(loaded.missing_pricing_model.as_deref(), Some(model));
    }
}
