use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use ccusage_adapter_common::units::UnitError;
use ccusage_core::{Diagnostics, Skip, SkipKind};
use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;

use crate::{
    LoadedEntry, PricingMap, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    apply_total_token_fallback, calculate_cost_for_usage_at, cli::CostMode, fast::LinePrefilter,
    format_date_tz, missing_pricing_model_for_candidates,
};
use ccusage_adapter_common::jsonl;

const DEFAULT_MODEL: &str = "kimi-for-coding";
const DEFAULT_PROVIDER: &str = "moonshot";
// NOTE(2026-05-25, #1106): kimi-for-coding served kimi-k2.5 until 2026-04-20T15:28:10.072Z and kimi-k2.6 after.
const KIMI_FOR_CODING_K2_6_FROM_MS: i64 = 1_776_698_890_072;

/// Kimi `config.json` document, used to read the configured display model.
#[derive(Debug, Deserialize)]
struct KimiConfig {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    model: Option<String>,
}

/// A single Kimi wire JSONL line. Only the fields ccusage consumes are declared;
/// serde skips everything else.
#[derive(Debug, Deserialize)]
struct KimiWireLine {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    r#type: Option<String>,
    // old format
    message: Option<KimiWireMessage>,
    #[serde(default, deserialize_with = "jsonl::lenient_f64")]
    timestamp: Option<f64>,
    // new Kimi Code format
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    model: Option<String>,
    usage: Option<KimiCodeUsage>,
    #[serde(
        rename = "usageScope",
        default,
        deserialize_with = "jsonl::non_empty_string"
    )]
    usage_scope: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_i64")]
    time: Option<i64>,
}

/// The `message` block carried by a Kimi wire line.
#[derive(Debug, Deserialize)]
struct KimiWireMessage {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    r#type: Option<String>,
    payload: Option<KimiWirePayload>,
}

/// The `message.payload` block carrying token usage and the message id.
#[derive(Debug, Deserialize)]
struct KimiWirePayload {
    token_usage: Option<KimiTokenUsage>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    message_id: Option<String>,
}

/// Token counts reported under `message.payload.token_usage`.
#[derive(Debug, Default, Deserialize)]
struct KimiTokenUsage {
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    input_other: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    output: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    input_cache_creation: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    input_cache_read: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    total: u64,
}

#[derive(Debug, Default, Deserialize)]
struct KimiCodeUsage {
    #[serde(
        rename = "inputOther",
        default,
        deserialize_with = "jsonl::lenient_u64"
    )]
    input_other: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    output: u64,
    #[serde(
        rename = "inputCacheCreation",
        default,
        deserialize_with = "jsonl::lenient_u64"
    )]
    input_cache_creation: u64,
    #[serde(
        rename = "inputCacheRead",
        default,
        deserialize_with = "jsonl::lenient_u64"
    )]
    input_cache_read: u64,
}

#[derive(Debug, Clone)]
pub(super) struct KimiUsageEntry {
    timestamp: TimestampMs,
    timestamp_text: String,
    session_id: String,
    model: String,
    message_id: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    extra_total_tokens: u64,
}

pub(super) fn read_wire_file(
    path: &Path,
    diagnostics: &Diagnostics,
) -> std::result::Result<Vec<KimiUsageEntry>, UnitError> {
    let model = read_model_from_config(path);
    let content = fs::read(path).map_err(UnitError::from)?;
    // Usable Kimi wire lines carry either `token_usage` (old format) or `usage.record` (new).
    let prefilter = LinePrefilter::any(&[br#""token_usage""#, br#""usage.record""#]);
    Ok(
        jsonl::numbered_records_with_usage_diagnostics::<KimiWireLine>(
            &content,
            Some(&prefilter),
            path,
            diagnostics,
            is_usage_envelope,
        )
        .filter_map(|(number, line)| wire_line_to_entry(&line, path, &model, number, diagnostics))
        .collect::<Vec<_>>(),
    )
}

fn is_usage_envelope(value: &serde_json::Value) -> bool {
    let has_new_usage = value.get("type").and_then(serde_json::Value::as_str)
        == Some("usage.record")
        && value.get("usageScope").and_then(serde_json::Value::as_str) == Some("turn")
        && value.get("usage").is_some();
    let has_old_usage = value
        .get("message")
        .and_then(|message| message.get("type"))
        .and_then(serde_json::Value::as_str)
        == Some("StatusUpdate")
        && value
            .get("message")
            .and_then(|message| message.get("payload"))
            .and_then(|payload| payload.get("token_usage"))
            .is_some();
    has_new_usage || has_old_usage
}

fn read_model_from_config(file_path: &Path) -> String {
    let Some(root) = kimi_root_from_wire_path(file_path) else {
        return DEFAULT_MODEL.to_string();
    };
    let Ok(content) = fs::read_to_string(root.join("config.json")) else {
        return DEFAULT_MODEL.to_string();
    };
    let Ok(config) = serde_json::from_str::<KimiConfig>(&content) else {
        return DEFAULT_MODEL.to_string();
    };
    config.model.unwrap_or_else(|| DEFAULT_MODEL.to_string())
}

fn kimi_root_from_wire_path(file_path: &Path) -> Option<PathBuf> {
    // Old layout: root/sessions/<group>/<session>/wire.jsonl
    // New layout: root/sessions/<ws>/<session>/agents/<agent>/wire.jsonl
    let agent_dir = file_path.parent()?;
    if agent_dir.parent()?.file_name()?.to_str() == Some("agents") {
        agent_dir
            .parent()?
            .parent()?
            .parent()?
            .parent()?
            .parent()
            .map(Path::to_path_buf)
    } else {
        file_path
            .parent()?
            .parent()?
            .parent()?
            .parent()
            .map(Path::to_path_buf)
    }
}

fn wire_line_to_entry(
    line: &KimiWireLine,
    file_path: &Path,
    model: &str,
    number: u32,
    diagnostics: &Diagnostics,
) -> Option<KimiUsageEntry> {
    match line.r#type.as_deref() {
        Some("usage.record") => wire_line_to_entry_new(line, file_path, number, diagnostics),
        Some("metadata") => None,
        _ => wire_line_to_entry_old(line, file_path, model, number, diagnostics),
    }
}

fn wire_line_to_entry_new(
    line: &KimiWireLine,
    file_path: &Path,
    number: u32,
    diagnostics: &Diagnostics,
) -> Option<KimiUsageEntry> {
    // Only aggregate turn-level records; session records are cumulative totals.
    if line.usage_scope.as_deref() != Some("turn") {
        return None;
    }
    let usage_counts = line.usage.as_ref()?;
    let usage = TokenUsageRaw {
        input_tokens: usage_counts.input_other,
        output_tokens: usage_counts.output,
        cache_creation_input_tokens: usage_counts.input_cache_creation,
        cache_read_input_tokens: usage_counts.input_cache_read,
        speed: None,
        cache_creation: None,
    };
    if crate::total_usage_tokens(usage) == 0 {
        return None;
    }
    let timestamp = required_wire_timestamp(
        line.time.and_then(TimestampMs::from_instant_millis),
        file_path,
        number,
        diagnostics,
    )?;
    let model = line.model.as_deref().unwrap_or(DEFAULT_MODEL);
    let model = model.trim_prefix("kimi-code/").to_string();
    Some(KimiUsageEntry {
        timestamp,
        timestamp_text: crate::format_rfc3339_millis(timestamp),
        session_id: extract_session_id(file_path),
        model,
        message_id: None,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_creation_tokens: usage.cache_creation_input_tokens,
        cache_read_tokens: usage.cache_read_input_tokens,
        extra_total_tokens: 0,
    })
}

fn wire_line_to_entry_old(
    line: &KimiWireLine,
    file_path: &Path,
    model: &str,
    number: u32,
    diagnostics: &Diagnostics,
) -> Option<KimiUsageEntry> {
    let message = line.message.as_ref()?;
    if message.r#type.as_deref() != Some("StatusUpdate") {
        return None;
    }
    let payload = message.payload.as_ref()?;
    let token_usage = payload.token_usage.as_ref()?;
    let usage = TokenUsageRaw {
        input_tokens: token_usage.input_other,
        output_tokens: token_usage.output,
        cache_creation_input_tokens: token_usage.input_cache_creation,
        cache_read_input_tokens: token_usage.input_cache_read,
        speed: None,
        cache_creation: None,
    };
    let (usage, extra_total_tokens) = apply_total_token_fallback(usage, 0, token_usage.total);
    if crate::total_usage_tokens(usage) + extra_total_tokens == 0 {
        return None;
    }
    let timestamp = required_wire_timestamp(
        line.timestamp.and_then(timestamp_from_seconds),
        file_path,
        number,
        diagnostics,
    )?;
    Some(KimiUsageEntry {
        timestamp,
        timestamp_text: crate::format_rfc3339_millis(timestamp),
        session_id: extract_session_id(file_path),
        model: model.to_string(),
        message_id: payload.message_id.clone(),
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_creation_tokens: usage.cache_creation_input_tokens,
        cache_read_tokens: usage.cache_read_input_tokens,
        extra_total_tokens,
    })
}

fn required_wire_timestamp(
    timestamp: Option<TimestampMs>,
    path: &Path,
    number: u32,
    diagnostics: &Diagnostics,
) -> Option<TimestampMs> {
    if timestamp.is_none() {
        diagnostics.record(Skip::record(
            path,
            number,
            SkipKind::InvalidTimestamp,
            "Kimi usage has no valid timestamp",
        ));
    }
    timestamp
}

fn timestamp_from_seconds(seconds: f64) -> Option<TimestampMs> {
    if !seconds.is_finite() {
        return None;
    }
    let millis = (seconds * 1000.0).trunc();
    if millis < i64::MIN as f64 || millis > i64::MAX as f64 {
        return None;
    }
    TimestampMs::from_instant_millis(millis as i64)
}

fn extract_session_id(file_path: &Path) -> String {
    // Old layout: sessions/<group>/<session>/wire.jsonl
    // New layout: sessions/<ws>/<session>/agents/<agent>/wire.jsonl
    let parent = file_path.parent();
    let session_dir = if parent
        .and_then(|p| p.parent())
        .and_then(Path::file_name)
        .and_then(|n| n.to_str())
        == Some("agents")
    {
        parent.and_then(|p| p.parent()).and_then(|p| p.parent())
    } else {
        parent
    };
    session_dir
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("unknown")
        .to_string()
}

#[derive(Debug, PartialEq, Eq, Hash)]
pub(super) struct KimiEntryKey {
    session_id: String,
    message_id: Option<String>,
    timestamp: TimestampMs,
    model: String,
    tokens: [u64; 5],
}

impl KimiUsageEntry {
    pub(super) fn dedupe_key(&self) -> KimiEntryKey {
        KimiEntryKey {
            session_id: self.session_id.clone(),
            message_id: self.message_id.clone(),
            timestamp: self.timestamp,
            model: self.model.clone(),
            tokens: [
                self.input_tokens,
                self.output_tokens,
                self.cache_creation_tokens,
                self.cache_read_tokens,
                self.extra_total_tokens,
            ],
        }
    }
}

pub(super) fn kimi_entry_to_loaded(
    entry: KimiUsageEntry,
    tz: &JiffTimeZone,
    mode: CostMode,
    pricing: &PricingMap,
) -> LoadedEntry {
    let usage = TokenUsageRaw {
        input_tokens: entry.input_tokens,
        output_tokens: entry.output_tokens,
        cache_creation_input_tokens: entry.cache_creation_tokens,
        cache_read_input_tokens: entry.cache_read_tokens,
        speed: None,
        cache_creation: None,
    };
    let cost = calculate_kimi_cost(&entry, mode, pricing, usage);
    let missing_pricing_model = missing_kimi_pricing(&entry, mode, pricing, usage);
    let data = UsageEntry {
        session_id: Some(entry.session_id.clone()),
        timestamp: entry.timestamp_text,
        version: None,
        message: UsageMessage {
            usage,
            model: Some(entry.model.clone()),
            id: entry.message_id.clone(),
        },
        cost_usd: None,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };
    LoadedEntry {
        date: format_date_tz(entry.timestamp, tz),
        timestamp: entry.timestamp,
        project: Arc::from("kimi"),
        session_id: Arc::from(entry.session_id),
        project_path: Arc::from("Kimi"),
        cost,
        extra_total_tokens: entry.extra_total_tokens,
        credits: None,
        message_count: None,
        model: Some(entry.model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        data,
        stored_cost: None,
    }
}

fn calculate_kimi_cost(
    entry: &KimiUsageEntry,
    mode: CostMode,
    pricing: &PricingMap,
    usage: TokenUsageRaw,
) -> f64 {
    match mode {
        CostMode::Display => 0.0,
        CostMode::Auto | CostMode::Calculate => {
            for candidate in model_candidates(entry) {
                if pricing.find(&candidate).is_some() {
                    return calculate_cost_for_usage_at(
                        Some(&candidate),
                        usage,
                        None,
                        Some(entry.timestamp),
                        CostMode::Calculate,
                        Some(pricing),
                    );
                }
            }
            0.0
        }
    }
}

fn missing_kimi_pricing(
    entry: &KimiUsageEntry,
    mode: CostMode,
    pricing: &PricingMap,
    usage: TokenUsageRaw,
) -> Option<String> {
    if mode == CostMode::Display {
        return None;
    }
    missing_pricing_model_for_candidates(
        &entry.model,
        model_candidates(entry),
        crate::total_usage_tokens(usage).saturating_add(entry.extra_total_tokens),
        Some(pricing),
    )
}

fn model_candidates(entry: &KimiUsageEntry) -> Vec<String> {
    let mut candidates = Vec::new();
    if entry.model == DEFAULT_MODEL {
        candidates.push(kimi_for_coding_pricing_model(entry.timestamp).to_string());
    }
    candidates.extend([
        format!("{DEFAULT_PROVIDER}/{}", entry.model),
        format!("kimi/{}", entry.model),
        entry.model.clone(),
    ]);
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|candidate| seen.insert(candidate.clone()));
    candidates
}

fn kimi_for_coding_pricing_model(timestamp: TimestampMs) -> &'static str {
    if timestamp < TimestampMs::from_millis(KIMI_FOR_CODING_K2_6_FROM_MS) {
        "moonshot/kimi-k2.5"
    } else {
        "moonshot/kimi-k2.6"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::fs_fixture;

    #[test]
    fn diagnoses_both_wire_formats_without_a_timestamp() {
        let fixture = ccusage_test_support::fs_fixture!({
            "sessions/group/session/wire.jsonl": concat!(
                r#"{"type":"usage.record","usageScope":"turn","usage":{"inputOther":10,"output":2}}"#,
                "\n",
                r#"{"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":10,"output":2}}}}"#,
            ),
        });
        let diagnostics = Diagnostics::new("kimi");
        let events = read_wire_file(
            &fixture.path("sessions/group/session/wire.jsonl"),
            &diagnostics,
        )
        .unwrap();
        assert!(events.is_empty());
        assert_eq!(
            diagnostics.skipped(ccusage_core::SkipKind::InvalidTimestamp),
            2
        );
    }
    #[test]
    fn kimi_root_resolves_correctly_for_both_path_layouts() {
        let fixture = fs_fixture!({
            "sessions/group/session-a/wire.jsonl": "",
            "sessions/ws/session-b/agents/agent-1/wire.jsonl": "",
        });
        let old_path = fixture.path("sessions/group/session-a/wire.jsonl");
        let new_path = fixture.path("sessions/ws/session-b/agents/agent-1/wire.jsonl");

        let old_root = kimi_root_from_wire_path(&old_path).unwrap();
        let new_root = kimi_root_from_wire_path(&new_path).unwrap();

        assert_eq!(old_root, fixture.root().to_path_buf());
        assert_eq!(new_root, fixture.root().to_path_buf());
    }

    #[test]
    fn falls_back_to_total_tokens_when_kimi_parts_are_missing() {
        let fixture = fs_fixture!({
            "sessions/group/session-a/wire.jsonl": "",
        });
        let file = fixture.path("sessions/group/session-a/wire.jsonl");
        let line = serde_json::from_value::<KimiWireLine>(serde_json::json!({
            "timestamp": 1770983427.123,
            "message": {
                "type": "StatusUpdate",
                "payload": {
                    "token_usage": {
                        "total": 432
                    }
                }
            }
        }))
        .unwrap();

        let entry =
            wire_line_to_entry(&line, &file, "kimi-k2", 1, &Diagnostics::new("kimi")).unwrap();

        assert_eq!(entry.output_tokens, 432);
        assert_eq!(entry.extra_total_tokens, 0);
    }

    #[test]
    fn prices_default_kimi_model_by_timestamp_without_changing_display_model() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = PricingMap::load_embedded();
        let usage = TokenUsageRaw {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: 20,
            cache_read_input_tokens: 10,
            speed: None,
            cache_creation: None,
        };
        let before_cutoff = KimiUsageEntry {
            timestamp: TimestampMs::from_millis(1_776_698_890_071),
            timestamp_text: "2026-04-20T15:28:10.071Z".to_string(),
            session_id: "session-a".to_string(),
            model: DEFAULT_MODEL.to_string(),
            message_id: Some("msg-before".to_string()),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_creation_tokens: usage.cache_creation_input_tokens,
            cache_read_tokens: usage.cache_read_input_tokens,
            extra_total_tokens: 0,
        };
        let at_cutoff = KimiUsageEntry {
            timestamp: TimestampMs::from_millis(1_776_698_890_072),
            timestamp_text: "2026-04-20T15:28:10.072Z".to_string(),
            message_id: Some("msg-at".to_string()),
            ..before_cutoff.clone()
        };

        let before_cost = calculate_kimi_cost(&before_cutoff, CostMode::Calculate, &pricing, usage);
        let at_cost = calculate_kimi_cost(&at_cutoff, CostMode::Calculate, &pricing, usage);
        let loaded = kimi_entry_to_loaded(
            at_cutoff,
            &jiff::tz::TimeZone::system(),
            CostMode::Calculate,
            &pricing,
        );

        assert!((before_cost - 0.000226).abs() < f64::EPSILON);
        assert!((at_cost - 0.00032035).abs() < f64::EPSILON);
        assert_eq!(loaded.model.as_deref(), Some(DEFAULT_MODEL));
        assert_eq!(loaded.data.message.model.as_deref(), Some(DEFAULT_MODEL));
    }
}
