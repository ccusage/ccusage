use std::{path::Path, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;
use serde_json::Value;

use crate::{
    LoadedEntry, PricingMap, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    calculate_cost_for_usage, cli::CostMode, format_date_tz, format_rfc3339_millis,
    missing_pricing_model_for_candidates,
};

/// One parsed session log: the model-completed records plus the project root
/// the session announced in its metadata record.
pub(super) struct MuseSession {
    pub(super) entries: Vec<MuseEntry>,
    pub(super) workspace_root: Option<String>,
}

pub(super) struct MuseEntry {
    pub(super) timestamp: TimestampMs,
    timestamp_text: String,
    pub(super) session_id: String,
    pub(super) model: String,
    usage: TokenUsageRaw,
    pub(super) record_id: String,
}

/// Parses one `session.jsonl` event-sourced stream into one `MuseEntry` per
/// `model_completed` record that carries token metrics. `recorded_at` is
/// microseconds since the Unix epoch, which is why it is divided by 1000 to
/// reach the millisecond `TimestampMs` every report expects.
pub(super) fn parse_session_file(contents: &str) -> MuseSession {
    let mut entries = Vec::new();
    let mut workspace_root = None;
    for line in contents.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match value.get("payload_type").and_then(|v| v.as_str()) {
            Some("runtime.session") => {
                if let Some(entry) = parse_model_completed(&value) {
                    entries.push(entry);
                }
            }
            Some("runtime.session.metadata") => {
                if workspace_root.is_none()
                    && let Some(root) = value
                        .get("payload")
                        .and_then(|p| p.get("record"))
                        .and_then(|r| r.get("workspace_root"))
                        .and_then(|v| v.as_str())
                {
                    workspace_root = Some(root.to_string());
                }
            }
            // Tombstones and every other payload type carry no usage.
            _ => {}
        }
    }
    MuseSession {
        entries,
        workspace_root,
    }
}

fn parse_model_completed(envelope: &Value) -> Option<MuseEntry> {
    let event = envelope.get("payload")?.get("event")?;
    if event.get("kind").and_then(|v| v.as_str()) != Some("model_completed") {
        return None;
    }
    let model = event
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if model.is_empty() {
        return None;
    }
    let usage = event.get("usage").map(parse_usage).unwrap_or_default();
    if usage.input_tokens == 0
        && usage.output_tokens == 0
        && usage.cache_read_input_tokens == 0
        && usage.cache_creation_input_tokens == 0
    {
        return None;
    }
    let recorded_at = envelope.get("recorded_at").and_then(|v| v.as_u64())?;
    if recorded_at == 0 {
        return None;
    }
    let session_id = envelope
        .get("stream")
        .and_then(|s| s.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let record_id = envelope
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let timestamp = TimestampMs::from_millis((recorded_at / 1000) as i64);
    Some(MuseEntry {
        timestamp,
        timestamp_text: format_rfc3339_millis(timestamp),
        session_id,
        model,
        usage,
        record_id,
    })
}

/// Muse reports `input_tokens` gross — the cached prefix is part of it — so it
/// must be netted before storing or the cached tokens would be counted twice
/// against the input rate. `output_tokens` is gross too, with
/// `reasoning_tokens` a subset of it, which is how every other adapter treats
/// reasoning and costs it at the output rate.
fn parse_usage(usage: &Value) -> TokenUsageRaw {
    let cache_read = usage
        .get("cache_read_tokens")
        .and_then(|v| v.as_u64())
        .or_else(|| usage.get("cached_tokens").and_then(|v| v.as_u64()))
        .unwrap_or(0);
    let gross_input = usage
        .get("input_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    TokenUsageRaw {
        input_tokens: gross_input.saturating_sub(cache_read),
        output_tokens: usage
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        cache_read_input_tokens: cache_read,
        cache_creation_input_tokens: usage
            .get("cache_write_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        speed: None,
        cache_creation: None,
    }
}

pub(super) fn to_loaded_entry(
    entry: MuseEntry,
    tz: Option<&JiffTimeZone>,
    pricing: &PricingMap,
    workspace_root: Option<&str>,
) -> LoadedEntry {
    let project = workspace_root
        .and_then(|root| Path::new(root).file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "muse".to_string());
    let cost = calculate_muse_cost(&entry, pricing);
    let missing_pricing_model = missing_muse_pricing(&entry, pricing);
    let data = UsageEntry {
        session_id: Some(entry.session_id.clone()),
        timestamp: entry.timestamp_text.clone(),
        version: None,
        message: UsageMessage {
            usage: entry.usage,
            model: Some(entry.model.clone()),
            id: Some(format!("muse:{}", entry.record_id)),
        },
        cost_usd: None,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };
    LoadedEntry {
        date: format_date_tz(entry.timestamp, tz),
        timestamp: entry.timestamp,
        project: Arc::from(project.as_str()),
        session_id: Arc::from(entry.session_id.as_str()),
        project_path: Arc::from(project.as_str()),
        cost,
        credits: None,
        extra_total_tokens: 0,
        message_count: None,
        model: Some(entry.model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        data,
    }
}

/// Muse logs no cost, so every entry is priced from the pricing map by model.
fn calculate_muse_cost(entry: &MuseEntry, pricing: &PricingMap) -> f64 {
    let cost = calculate_cost_for_usage(
        Some(&entry.model),
        entry.usage,
        None,
        CostMode::Calculate,
        Some(pricing),
    );
    if cost.is_finite() && cost > 0.0 {
        cost
    } else {
        0.0
    }
}

fn missing_muse_pricing(entry: &MuseEntry, pricing: &PricingMap) -> Option<String> {
    missing_pricing_model_for_candidates(
        &entry.model,
        std::iter::once(entry.model.clone()),
        crate::total_usage_tokens(entry.usage),
        Some(pricing),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::fs_fixture;
    use std::fs;

    fn envelope(
        payload_type: &str,
        id: &str,
        stream_id: &str,
        recorded_at_us: u64,
        event: &str,
    ) -> String {
        format!(
            r#"{{"schema_version":1,"id":"{id}","stream":{{"kind":"session","id":"{stream_id}"}},"sequence":1,"recorded_at":{recorded_at_us},"record_type":"event","durability":"durable","causation_id":null,"payload_type":"{payload_type}","payload_schema_version":1,"payload":{event}}}"#
        )
    }

    #[test]
    fn parses_model_completed_records_with_netted_tokens() {
        let contents = [
            envelope(
                "runtime.session.metadata",
                "meta-1",
                "sess-a",
                1_785_962_827_173_826,
                r#"{"record":{"workspace_root":"/home/user/projects/ccusage"}}"#,
            ),
            envelope(
                "runtime.session",
                "rec-2",
                "sess-a",
                1_785_962_827_173_826,
                r#"{"event":{"kind":"model_completed","model":"muse-spark-1.2","usage":{"cache_read_tokens":15665,"cache_write_tokens":0,"cached_tokens":15665,"input_tokens":15924,"output_tokens":355,"reasoning_tokens":111}},"kind":"run"}"#,
            ),
            envelope(
                "runtime.session",
                "rec-3",
                "sess-a",
                1_785_962_828_000_000,
                r#"{"event":{"kind":"started","prompt":"hi"}}"#,
            ),
        ]
        .join("\n");

        let session = parse_session_file(&contents);

        assert_eq!(
            session.workspace_root.as_deref(),
            Some("/home/user/projects/ccusage")
        );
        assert_eq!(session.entries.len(), 1);
        let entry = &session.entries[0];
        assert_eq!(entry.session_id, "sess-a");
        assert_eq!(entry.model, "muse-spark-1.2");
        assert_eq!(entry.usage.input_tokens, 259);
        assert_eq!(entry.usage.cache_read_input_tokens, 15665);
        assert_eq!(entry.usage.output_tokens, 355);
        assert_eq!(entry.usage.cache_creation_input_tokens, 0);
        assert_eq!(entry.timestamp.as_millis(), 1_785_962_827_173);
    }

    #[test]
    fn falls_back_to_cached_tokens_and_skips_zero_usage_records() {
        let contents = [
            envelope(
                "runtime.session",
                "rec-1",
                "sess-a",
                1_785_962_827_173_826,
                r#"{"event":{"kind":"model_completed","model":"muse-spark-1.2-contributor","usage":{"cache_write_tokens":0,"input_tokens":100,"output_tokens":10,"reasoning_tokens":0}}}"#,
            ),
            envelope(
                "runtime.session",
                "rec-2",
                "sess-a",
                1_785_962_827_173_826,
                r#"{"event":{"kind":"model_completed","model":"muse-spark-1.2","usage":{"input_tokens":0,"output_tokens":0}}}"#,
            ),
        ]
        .join("\n");

        let session = parse_session_file(&contents);

        assert_eq!(session.entries.len(), 1);
        assert_eq!(session.entries[0].usage.input_tokens, 100);
        assert_eq!(session.entries[0].usage.cache_read_input_tokens, 0);
    }

    #[test]
    fn ignores_tombstone_lines_and_non_json_lines() {
        let contents = [
            r#"{"retained_marker":"omitted_live_only","stream":{"kind":"session","id":"sess-a"},"position":1,"omitted_record":{}}"#
                .to_string(),
            "not json".to_string(),
            envelope(
                "runtime.session",
                "rec-1",
                "sess-a",
                1_785_962_827_173_826,
                r#"{"event":{"kind":"model_completed","model":"muse-spark-1.2","usage":{"input_tokens":5,"output_tokens":2}}}"#,
            ),
        ]
        .join("\n");

        let session = parse_session_file(&contents);

        assert_eq!(session.entries.len(), 1);
        assert_eq!(session.entries[0].model, "muse-spark-1.2");
    }

    #[test]
    fn maps_workspace_root_to_project_name() {
        let fixture = fs_fixture!({
            "muse/sessions/2026/08/01/11111111-1111-1111-1111-111111111111/session.jsonl": [
                envelope(
                    "runtime.session.metadata",
                    "meta-1",
                    "11111111-1111-1111-1111-111111111111",
                    1_785_962_827_173_826,
                    r#"{"record":{"workspace_root":"/home/user/projects/ccusage"}}"#,
                ),
                envelope(
                    "runtime.session",
                    "rec-2",
                    "11111111-1111-1111-1111-111111111111",
                    1_785_962_827_173_826,
                    r#"{"event":{"kind":"model_completed","model":"muse-spark-1.2","usage":{"input_tokens":100,"output_tokens":10}}}"#,
                ),
            ]
            .join("\n"),
        });

        let _xdg = ccusage_test_support::EnvVarGuard::set("XDG_DATA_HOME", fixture.root());
        let files = crate::paths::muse_session_files().unwrap();
        assert_eq!(files.len(), 1);
        let contents = fs::read_to_string(&files[0]).unwrap();
        let session = parse_session_file(&contents);
        let entry = to_loaded_entry(
            session.entries.into_iter().next().unwrap(),
            Some(&jiff::tz::TimeZone::UTC),
            &PricingMap::default(),
            session.workspace_root.as_deref(),
        );

        assert_eq!(entry.project.as_ref(), "ccusage");
        assert_eq!(entry.project_path.as_ref(), "ccusage");
        assert_eq!(entry.date, "2026-08-05");
    }
}
