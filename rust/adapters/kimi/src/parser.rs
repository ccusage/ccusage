use ccusage_core::cost::PriceTable;
use ccusage_core::{
    cost::{EntryPricing, UsageGranularity, price_entry},
    pricing::MatchKind,
};
use std::{path::Path, sync::Arc};

use ccusage_adapter_common::units::UnitError;
use ccusage_core::{Diagnostics, Skip, SkipKind};
use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;

use crate::{LoadedEntry, TimestampMs, TokenUsageRaw, date_at, fast::LinePrefilter};
use ccusage_adapter_common::jsonl;

const DEFAULT_MODEL: &str = "kimi-for-coding";
const DEFAULT_PROVIDER: &str = "moonshot";
// NOTE(2026-05-25, #1106): The `kimi-for-coding` plan switched from Kimi K2.5 to K2.6 at 2026-04-20T15:28:10.072Z. Use the model the plan served at the recorded time for pricing.
const KIMI_FOR_CODING_K2_6_FROM_MS: i64 = 1_776_698_890_072;

#[derive(Debug, Deserialize)]
struct KimiConfig {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct KimiWireLine {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    r#type: Option<String>,
    message: Option<KimiWireMessage>,
    #[serde(default, deserialize_with = "jsonl::lenient_f64")]
    timestamp: Option<f64>,
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

#[derive(Debug, Deserialize)]
struct KimiWireMessage {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    r#type: Option<String>,
    payload: Option<KimiWirePayload>,
}

#[derive(Debug, Deserialize)]
struct KimiWirePayload {
    token_usage: Option<KimiTokenUsage>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    message_id: Option<String>,
}

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

    session_id: String,
    model: String,
    logged_model: Option<String>,
    message_id: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    report_remainder: u64,
}

use super::paths::WireFile;

pub(super) fn read_wire_file(
    file: &WireFile,
    diagnostics: &Diagnostics,
) -> std::result::Result<Vec<KimiUsageEntry>, UnitError> {
    let path = file.as_ref();
    let prefilter = LinePrefilter::any(&[br#""token_usage""#, br#""usage.record""#]);
    let mut entries = Vec::new();
    jsonl::read_records::<KimiWireLine>(
        path,
        Some(&prefilter),
        diagnostics,
        is_usage_envelope,
        |number, line, diagnostics| {
            entries.extend(wire_line_to_entry(&line, file, number, diagnostics));
        },
    )?;
    Ok(entries)
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

fn read_model_from_config(root: Option<&Path>) -> String {
    let Some(root) = root else {
        return DEFAULT_MODEL.to_string();
    };
    let Ok(config) =
        ccusage_adapter_common::units::read_utf8_json::<KimiConfig>(&root.join("config.json"))
    else {
        return DEFAULT_MODEL.to_string();
    };
    config.model.unwrap_or_else(|| DEFAULT_MODEL.to_string())
}

fn wire_line_to_entry(
    line: &KimiWireLine,
    file: &WireFile,
    number: u32,
    diagnostics: &Diagnostics,
) -> Option<KimiUsageEntry> {
    match line.r#type.as_deref() {
        Some("usage.record") => entry_from_usage_record(line, file, number, diagnostics),
        Some("metadata") => None,
        _ => entry_from_status_update(line, file, number, diagnostics),
    }
}

fn entry_from_usage_record(
    line: &KimiWireLine,
    file: &WireFile,
    number: u32,
    diagnostics: &Diagnostics,
) -> Option<KimiUsageEntry> {
    let file_path = file.as_ref();
    // NOTE(2026-07-09, #1362): Kimi Code writes cumulative `usageScope: session` records alongside individual turns. Counting both would duplicate the turn usage.
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
    let Some(counted_tokens) = ccusage_core::checked_total_usage_tokens(usage, 0) else {
        diagnostics.record(Skip::record(
            file_path,
            number,
            SkipKind::InvalidData,
            "Kimi token counts exceed the u64 range",
        ));
        return None;
    };
    if counted_tokens == 0 {
        return None;
    }
    let timestamp = required_wire_timestamp(
        line.time.and_then(TimestampMs::from_instant_millis),
        file_path,
        number,
        diagnostics,
    )?;
    let logged = line.model.as_deref().unwrap_or(DEFAULT_MODEL);
    let model = logged.trim_prefix("kimi-code/").to_string();
    let logged_model = line
        .model
        .as_deref()
        .filter(|logged| *logged != model || model == DEFAULT_MODEL)
        .map(str::to_owned);
    Some(KimiUsageEntry {
        timestamp,

        session_id: file.session_id.clone(),
        model,
        logged_model,
        message_id: None,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_creation_tokens: usage.cache_creation_input_tokens,
        cache_read_tokens: usage.cache_read_input_tokens,
        report_remainder: 0,
    })
}

fn entry_from_status_update(
    line: &KimiWireLine,
    file: &WireFile,
    number: u32,
    diagnostics: &Diagnostics,
) -> Option<KimiUsageEntry> {
    let file_path = file.as_ref();
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
    let report_remainder =
        ccusage_core::AdditionalTokens::from_total(usage, 0, token_usage.total).report_remainder;
    let Some(counted_tokens) = ccusage_core::checked_total_usage_tokens(usage, report_remainder)
    else {
        diagnostics.record(Skip::record(
            file_path,
            number,
            SkipKind::InvalidData,
            "Kimi token counts exceed the u64 range",
        ));
        return None;
    };
    if counted_tokens == 0 {
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

        session_id: file.session_id.clone(),
        model: file
            .config
            .model
            .get_or_init(|| read_model_from_config(file.config.root.as_deref()))
            .clone(),
        logged_model: None,
        message_id: payload.message_id.clone(),
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_creation_tokens: usage.cache_creation_input_tokens,
        cache_read_tokens: usage.cache_read_input_tokens,
        report_remainder,
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

#[derive(Debug, PartialEq, Eq, Hash)]
pub(super) struct KimiEntryKey {
    session_id: String,
    message_id: Option<String>,
    timestamp: TimestampMs,
    model: String,
    tokens: [u64; 5],
}

impl KimiUsageEntry {
    pub(super) fn billable_output_tokens(&self) -> u64 {
        self.output_tokens
    }

    pub(super) fn dedupe_key(&self) -> KimiEntryKey {
        let (output_tokens, remainder) = ccusage_core::AdditionalTokens {
            reasoning: 0,
            report_remainder: self.report_remainder,
        }
        .replay_counts(self.output_tokens);
        KimiEntryKey {
            session_id: self.session_id.clone(),
            message_id: self.message_id.clone(),
            timestamp: self.timestamp,
            model: self.model.clone(),
            tokens: [
                self.input_tokens,
                output_tokens,
                self.cache_creation_tokens,
                self.cache_read_tokens,
                remainder,
            ],
        }
    }

    fn usage(&self) -> TokenUsageRaw {
        TokenUsageRaw {
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_creation_input_tokens: self.cache_creation_tokens,
            cache_read_input_tokens: self.cache_read_tokens,
            speed: None,
            cache_creation: None,
        }
    }
}

pub(super) fn kimi_entry_to_loaded(
    entry: KimiUsageEntry,
    tz: &JiffTimeZone,
    pricing: PriceTable<'_>,
) -> LoadedEntry {
    let usage = entry.usage();
    let (cost, missing_pricing_model, estimated_pricing) = price_kimi_usage(&entry, pricing, usage);
    LoadedEntry {
        date: date_at(entry.timestamp, tz),
        timestamp: entry.timestamp,
        project: Arc::from("kimi"),
        session_id: Arc::from(entry.session_id),
        project_path: Arc::from("Kimi"),
        cost,
        extra_total_tokens: entry.report_remainder,
        credits: None,
        message_count: None,
        model: Some(entry.model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        estimated_pricing,
        usage,
        message_id: entry.message_id.clone(),
        version: None,
        recorded_cost: None,
        stored_cost: None,
    }
}

fn price_kimi_usage(
    entry: &KimiUsageEntry,
    pricing: PriceTable<'_>,
    usage: TokenUsageRaw,
) -> (f64, Option<String>, bool) {
    let names = model_candidates(entry);
    let candidates: Vec<_> = names
        .iter()
        .map(|(name, kind)| (name.as_str(), *kind))
        .collect();
    let priced = price_entry(
        EntryPricing {
            granularity: UsageGranularity::Request,
            usage,
            reasoning_tokens: 0,
            reported_model: Some(&entry.model),
            candidates: &candidates,
            stored_cost: None,
            timestamp: Some(entry.timestamp),
        },
        pricing,
    );
    (
        priced.cost,
        priced
            .missing_pricing
            .then(|| crate::model_aliases::resolve_model_name(&entry.model).into_owned()),
        priced.estimated,
    )
}

fn model_candidates(entry: &KimiUsageEntry) -> Vec<(String, MatchKind)> {
    let mut candidates = Vec::new();
    candidates.extend(
        entry
            .logged_model
            .clone()
            .map(|name| (name, MatchKind::Exact)),
    );
    let kind = if entry.model == DEFAULT_MODEL {
        candidates.push((
            kimi_for_coding_pricing_model(entry.timestamp).to_string(),
            MatchKind::Fuzzy,
        ));
        if entry.logged_model.is_some() {
            MatchKind::Exact
        } else {
            MatchKind::Fuzzy
        }
    } else {
        MatchKind::Fuzzy
    };
    candidates.extend([
        (format!("{DEFAULT_PROVIDER}/{}", entry.model), kind),
        (format!("kimi/{}", entry.model), kind),
        (entry.model.clone(), kind),
    ]);
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|(name, _)| seen.insert(name.clone()));
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
    fn invalid_utf8_in_an_ignored_config_field_preserves_the_default_model() {
        let fixture = ccusage_test_support::Fixture::new();
        std::fs::write(
            fixture.path("config.json"),
            b"{\"model\":\"configured\",\"ignored\":\"\xff\"}",
        )
        .unwrap();

        assert_eq!(read_model_from_config(Some(fixture.root())), DEFAULT_MODEL);
    }

    #[test]
    fn shares_the_config_model_per_root_and_loads_it_only_for_legacy_usage() {
        let legacy = r#"{"timestamp":1770983427,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":10}}}}"#;
        let modern = r#"{"type":"usage.record","model":"kimi-k2","usageScope":"turn","usage":{"inputOther":10},"time":1770983427000}"#;
        let fixture = fs_fixture!({
            "first/config.json": r#"{"model":"initial"}"#,
            "first/sessions/group/modern/wire.jsonl": modern,
            "first/sessions/group/old-a/wire.jsonl": legacy,
            "first/sessions/group/old-b/wire.jsonl": legacy,
            "second/config.json": r#"{"model":"other-root"}"#,
            "second/sessions/group/old/wire.jsonl": legacy,
        });
        let files = WireFile::from_paths(vec![
            fixture.path("first/sessions/group/modern/wire.jsonl"),
            fixture.path("first/sessions/group/old-a/wire.jsonl"),
            fixture.path("first/sessions/group/old-b/wire.jsonl"),
            fixture.path("second/sessions/group/old/wire.jsonl"),
        ]);
        let diagnostics = Diagnostics::new("kimi");
        assert_eq!(
            read_wire_file(&files[0], &diagnostics).unwrap()[0].model,
            "kimi-k2"
        );
        fixture.write_file("first/config.json", r#"{"model":"configured"}"#);
        assert_eq!(
            read_wire_file(&files[1], &diagnostics).unwrap()[0].model,
            "configured"
        );
        fixture.write_file("first/config.json", r#"{"model":"changed"}"#);
        assert_eq!(
            read_wire_file(&files[2], &diagnostics).unwrap()[0].model,
            "configured"
        );
        assert_eq!(
            read_wire_file(&files[3], &diagnostics).unwrap()[0].model,
            "other-root"
        );
    }

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
            &WireFile::from_paths(vec![fixture.path("sessions/group/session/wire.jsonl")])
                .remove(0),
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
    fn skips_and_diagnoses_token_counts_that_exceed_the_u64_range() {
        let fixture = ccusage_test_support::fs_fixture!({
            "sessions/group/session/wire.jsonl": concat!(
                r#"{"type":"usage.record","model":"kimi-k2","usageScope":"turn","usage":{"inputOther":18446744073709551615,"output":1},"time":1770983427000}"#,
                "\n",
                r#"{"type":"usage.record","model":"kimi-k2","usageScope":"turn","usage":{"inputOther":10,"output":2},"time":1770983428000}"#,
                "\n",
            ),
        });
        let diagnostics = Diagnostics::new("kimi");
        let events = read_wire_file(
            &WireFile::from_paths(vec![fixture.path("sessions/group/session/wire.jsonl")])
                .remove(0),
            &diagnostics,
        )
        .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].input_tokens, 10);
        assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
    }

    #[test]
    fn kimi_root_resolves_correctly_for_both_path_layouts() {
        let fixture = fs_fixture!({
            "sessions/group/session-a/wire.jsonl": "",
            "sessions/ws/session-b/agents/agent-1/wire.jsonl": "",
        });
        let old_path = fixture.path("sessions/group/session-a/wire.jsonl");
        let new_path = fixture.path("sessions/ws/session-b/agents/agent-1/wire.jsonl");

        let files = WireFile::from_paths(vec![old_path, new_path]);
        let old_root = files[0].config.root.clone().unwrap();
        let new_root = files[1].config.root.clone().unwrap();

        assert_eq!(old_root, fixture.root().to_path_buf());
        assert_eq!(new_root, fixture.root().to_path_buf());
    }

    #[test]
    fn keeps_total_only_usage_out_of_output_tokens() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
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

        let entry = wire_line_to_entry(
            &line,
            &WireFile::from_paths(vec![file]).remove(0),
            1,
            &Diagnostics::new("kimi"),
        )
        .unwrap();

        let entry =
            kimi_entry_to_loaded(entry, &JiffTimeZone::UTC, PriceTable::Calculate(&pricing));
        assert_eq!(entry.usage.output_tokens, 0);
        assert_eq!(entry.extra_total_tokens, 432);
        assert_eq!(entry.cost, 0.0);
    }

    #[test]
    fn prices_the_logged_wire_id_before_stripping_its_prefix() {
        ccusage_test_support::install_pricing_snapshots();
        let path = std::path::PathBuf::from("project/session/wire.jsonl");
        let file = WireFile::from_paths(vec![path]).pop().unwrap();
        let line = serde_json::from_str(
            r#"{"time":1794909600000,"usageScope":"turn","model":"kimi-code/test-model",
                "usage":{"inputOther":100}}"#,
        )
        .unwrap();
        let entry = entry_from_usage_record(&line, &file, 1, &Diagnostics::new("kimi")).unwrap();
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(
            r#"{
                "kimi-code/test-model":{"input_cost_per_token":1,"output_cost_per_token":1},
                "test-model":{"input_cost_per_token":2,"output_cost_per_token":2}
            }"#,
        );
        let loaded = kimi_entry_to_loaded(
            entry,
            &jiff::tz::TimeZone::UTC,
            PriceTable::Calculate(&pricing),
        );
        assert_eq!(loaded.model.as_deref(), Some("test-model"));
        assert_eq!(loaded.cost, 100.0);
    }

    #[test]
    fn prices_default_kimi_model_by_timestamp_without_changing_display_model() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
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

            session_id: "session-a".to_string(),
            model: DEFAULT_MODEL.to_string(),
            logged_model: None,
            message_id: Some("msg-before".to_string()),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_creation_tokens: usage.cache_creation_input_tokens,
            cache_read_tokens: usage.cache_read_input_tokens,
            report_remainder: 0,
        };
        let at_cutoff = KimiUsageEntry {
            timestamp: TimestampMs::from_millis(1_776_698_890_072),

            message_id: Some("msg-at".to_string()),
            ..before_cutoff.clone()
        };

        let before_cost =
            price_kimi_usage(&before_cutoff, PriceTable::Calculate(&pricing), usage).0;
        let at_cost = price_kimi_usage(&at_cutoff, PriceTable::Calculate(&pricing), usage).0;
        let loaded = kimi_entry_to_loaded(
            at_cutoff,
            &jiff::tz::TimeZone::UTC,
            PriceTable::Calculate(&pricing),
        );

        assert!((before_cost - 0.000226).abs() < f64::EPSILON);
        assert!((at_cost - 0.00032035).abs() < f64::EPSILON);
        assert_eq!(loaded.model.as_deref(), Some(DEFAULT_MODEL));
        assert_eq!(loaded.model.as_deref(), Some(DEFAULT_MODEL));
    }
}

#[cfg(test)]
mod pricing_identity_tests {
    use super::*;

    #[test]
    fn adapter_pricing_identity() {
        let entry = KimiUsageEntry {
            timestamp: ccusage_core::parse_ts_timestamp("2026-11-17T10:00:00Z").unwrap(),

            session_id: "s".into(),
            model: "kimi-for-coding".into(),
            logged_model: None,
            message_id: None,
            input_tokens: 1000,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            report_remainder: 0,
        };
        assert_eq!(
            model_candidates(&entry)
                .into_iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            [
                "moonshot/kimi-k2.6",
                "moonshot/kimi-for-coding",
                "kimi/kimi-for-coding",
                "kimi-for-coding"
            ]
        );
    }
}
