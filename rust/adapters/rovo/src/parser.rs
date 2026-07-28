use std::{fs, path::Path, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;

use crate::{
    LoadedEntry, PricingMap, Result, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    apply_total_token_fallback, calculate_cost_for_usage, cli::CostMode, cli_error, format_date_tz,
    missing_pricing_model_for_candidates,
};
use ccusage_adapter_common::jsonl;

/// Legacy (CLI 0.6.x) responses persist `model_name: null`; reports show this
/// placeholder and pricing falls through to the missing-pricing warning.
const DEFAULT_MODEL: &str = "unknown";
const PROVIDER_PREFIXES: &[&str] = &["anthropic", "openai"];

/// A Rovo Dev `session_context.json` document (one JSON object per session).
/// Only the fields ccusage consumes are declared; serde skips everything else.
#[derive(Debug, Deserialize)]
struct RovoSessionContext {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    id: Option<String>,
    /// Session creation time in Unix epoch seconds. Forked sessions inherit
    /// the parent's value, so it is only a fallback for per-turn timestamps.
    #[serde(default, deserialize_with = "jsonl::lenient_i64")]
    timestamp: Option<i64>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    workspace_path: Option<String>,
    #[serde(default)]
    message_history: Vec<serde_json::Value>,
}

/// A `message_history` entry. Usage is only carried by `kind: "response"`
/// entries; `kind: "request"` entries never have a `usage` object.
#[derive(Debug, Default, Deserialize)]
struct RovoMessage {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    kind: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    model_name: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    timestamp: Option<String>,
    /// Modern schema (pydantic-ai v1): e.g. `msg_vrtx_...` / `msg_bdrk_...`.
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    provider_response_id: Option<String>,
    /// Legacy schema (CLI 0.6.x) equivalent of `provider_response_id`.
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    vendor_id: Option<String>,
    usage: Option<RovoUsage>,
}

/// Token usage on a response entry. Modern records (pydantic-ai `RunUsage`)
/// carry `input_tokens`/`output_tokens`/`cache_*_tokens`; legacy records
/// (pydantic-ai `Usage`) carry `request_tokens`/`response_tokens`. Both
/// duplicate the raw provider values under `details`.
#[derive(Debug, Default, Deserialize)]
struct RovoUsage {
    /// Modern total prompt tokens; INCLUDES cache write and cache read.
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    input_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    output_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cache_write_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cache_read_tokens: u64,
    /// Legacy total prompt tokens; INCLUDES cache write and cache read.
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    request_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    response_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    total_tokens: u64,
    details: Option<RovoUsageDetails>,
}

/// Raw provider token counts; `input_tokens` here is the uncached share.
#[derive(Debug, Default, Deserialize)]
struct RovoUsageDetails {
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    input_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    output_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cache_creation_input_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cache_read_input_tokens: u64,
}

#[derive(Debug, Clone)]
pub(super) struct RovoUsageEntry {
    timestamp: TimestampMs,
    timestamp_text: String,
    session_id: String,
    workspace_path: Option<String>,
    model: String,
    response_id: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    extra_total_tokens: u64,
}

pub(super) fn read_session_file(path: &Path) -> Result<Vec<RovoUsageEntry>> {
    let content = fs::read(path)?;
    let context: RovoSessionContext = serde_json::from_slice(&content).map_err(|error| {
        cli_error(format!(
            "invalid Rovo Dev session file {}: {error}",
            path.display()
        ))
    })?;
    let session_id = extract_session_id(path, context.id.as_deref());
    let fallback_timestamp = context
        .timestamp
        .and_then(|seconds| seconds.checked_mul(1000))
        .map(TimestampMs::from_millis)
        .unwrap_or_else(|| file_modified_timestamp(path));
    Ok(context
        .message_history
        .into_iter()
        .filter_map(|value| serde_json::from_value::<RovoMessage>(value).ok())
        .filter_map(|message| {
            message_to_entry(
                &message,
                &session_id,
                context.workspace_path.as_deref(),
                fallback_timestamp,
            )
        })
        .collect())
}

fn message_to_entry(
    message: &RovoMessage,
    session_id: &str,
    workspace_path: Option<&str>,
    fallback_timestamp: TimestampMs,
) -> Option<RovoUsageEntry> {
    if message.kind.as_deref() != Some("response") {
        return None;
    }
    let usage_counts = message.usage.as_ref()?;
    let (usage, extra_total_tokens) = usage_to_raw(usage_counts);
    if crate::total_usage_tokens(usage) + extra_total_tokens == 0 {
        return None;
    }
    let timestamp = message
        .timestamp
        .as_deref()
        .and_then(parse_rovo_timestamp)
        .unwrap_or(fallback_timestamp);
    Some(RovoUsageEntry {
        timestamp,
        timestamp_text: crate::format_rfc3339_millis(timestamp),
        session_id: session_id.to_string(),
        workspace_path: workspace_path.map(str::to_string),
        model: message
            .model_name
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        response_id: message
            .provider_response_id
            .clone()
            .or_else(|| message.vendor_id.clone()),
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_creation_tokens: usage.cache_creation_input_tokens,
        cache_read_tokens: usage.cache_read_input_tokens,
        extra_total_tokens,
    })
}

fn usage_to_raw(usage: &RovoUsage) -> (TokenUsageRaw, u64) {
    // Both schemas duplicate the raw provider values under `details`, and the
    // top-level prompt total (`input_tokens` modern, `request_tokens` legacy)
    // already includes cache tokens — `details` is the exact split.
    let (input_tokens, cache_creation, cache_read) = match usage.details.as_ref() {
        Some(details) => (
            details.input_tokens,
            details.cache_creation_input_tokens,
            details.cache_read_input_tokens,
        ),
        None => (
            usage
                .input_tokens
                .max(usage.request_tokens)
                .saturating_sub(usage.cache_write_tokens)
                .saturating_sub(usage.cache_read_tokens),
            usage.cache_write_tokens,
            usage.cache_read_tokens,
        ),
    };
    let output_tokens = [
        usage.output_tokens,
        usage
            .details
            .as_ref()
            .map(|details| details.output_tokens)
            .unwrap_or(0),
        usage.response_tokens,
    ]
    .into_iter()
    .find(|tokens| *tokens > 0)
    .unwrap_or(0);
    let raw = TokenUsageRaw {
        input_tokens,
        output_tokens,
        cache_creation_input_tokens: cache_creation,
        cache_read_input_tokens: cache_read,
        speed: None,
        cache_creation: None,
    };
    apply_total_token_fallback(raw, 0, usage.total_tokens)
}

/// Rovo timestamps carry microsecond precision (`2026-02-23T21:03:45.664227Z`
/// or `...+00:00`), which `parse_ts_timestamp`'s fixed-width fast path does
/// not accept, so parse through jiff instead.
fn parse_rovo_timestamp(value: &str) -> Option<TimestampMs> {
    value
        .parse::<jiff::Timestamp>()
        .ok()
        .map(|timestamp| TimestampMs::from_millis(timestamp.as_millisecond()))
}

fn extract_session_id(file_path: &Path, context_id: Option<&str>) -> String {
    // The CLI itself treats the parent directory name as the session id.
    file_path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .or_else(|| context_id.map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

fn file_modified_timestamp(path: &Path) -> TimestampMs {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .map(TimestampMs::from_millis)
        .unwrap_or(TimestampMs::UNIX_EPOCH)
}

pub(super) fn rovo_entry_key(entry: &RovoUsageEntry) -> String {
    // Forked sessions copy the parent's message prefix verbatim, so the
    // provider response id alone identifies a response across session files.
    if let Some(response_id) = &entry.response_id {
        return format!("response:{response_id}");
    }
    // Legacy (CLI 0.6.x) responses carry no id at all, and a legacy fork's
    // copied prefix lives under a different session dir than the parent's, so
    // session_id must stay OUT of this fallback key or fork families
    // double-count. The millisecond-precision timestamp plus the full token
    // tuple is discriminating enough on its own.
    format!(
        "{}:{}:{}:{}:{}:{}:{}",
        entry.timestamp_text,
        entry.model,
        entry.input_tokens,
        entry.output_tokens,
        entry.cache_creation_tokens,
        entry.cache_read_tokens,
        entry.extra_total_tokens
    )
}

pub(super) fn rovo_entry_to_loaded(
    entry: RovoUsageEntry,
    tz: Option<&JiffTimeZone>,
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
    let cost = calculate_rovo_cost(&entry, mode, pricing, usage);
    let missing_pricing_model = missing_rovo_pricing(&entry, mode, pricing, usage);
    let data = UsageEntry {
        session_id: Some(entry.session_id.clone()),
        timestamp: entry.timestamp_text,
        version: None,
        message: UsageMessage {
            usage,
            model: Some(entry.model.clone()),
            id: entry.response_id.clone(),
        },
        cost_usd: None,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };
    LoadedEntry {
        date: format_date_tz(entry.timestamp, tz),
        timestamp: entry.timestamp,
        project: Arc::from("rovo"),
        session_id: Arc::from(entry.session_id),
        project_path: entry
            .workspace_path
            .as_deref()
            .map(Arc::from)
            .unwrap_or_else(|| Arc::from("Rovo Dev")),
        cost,
        extra_total_tokens: entry.extra_total_tokens,
        credits: None,
        message_count: None,
        model: Some(entry.model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        data,
    }
}

fn calculate_rovo_cost(
    entry: &RovoUsageEntry,
    mode: CostMode,
    pricing: &PricingMap,
    usage: TokenUsageRaw,
) -> f64 {
    match mode {
        // Rovo Dev bills Atlassian credits and persists no cost locally, so
        // display mode has nothing to show.
        CostMode::Display => 0.0,
        CostMode::Auto | CostMode::Calculate => {
            for candidate in model_candidates(entry) {
                if pricing.find(&candidate).is_some() {
                    return calculate_cost_for_usage(
                        Some(&candidate),
                        usage,
                        None,
                        CostMode::Calculate,
                        Some(pricing),
                    );
                }
            }
            0.0
        }
    }
}

fn missing_rovo_pricing(
    entry: &RovoUsageEntry,
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

fn model_candidates(entry: &RovoUsageEntry) -> Vec<String> {
    let mut candidates = vec![entry.model.clone()];
    candidates.extend(
        PROVIDER_PREFIXES
            .iter()
            .map(|prefix| format!("{prefix}/{}", entry.model)),
    );
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::fs_fixture;

    fn modern_session_json() -> String {
        serde_json::json!({
            "id": "wd2c0476-042b-49f3-b2d4-1a5c6bafe971",
            "timestamp": 1771880550,
            "workspace_path": "/home/user/project",
            "message_history": [
                {
                    "kind": "request",
                    "parts": [{"part_kind": "user-prompt", "content": "hello"}]
                },
                {
                    "kind": "response",
                    "model_name": "claude-sonnet-4-5-20250929",
                    "provider_name": "anthropic",
                    "provider_response_id": "msg_vrtx_014rqTwZTSMNrU4g6HcjDpQp",
                    "timestamp": "2026-02-23T21:03:45.664227Z",
                    "usage": {
                        "input_tokens": 7933,
                        "cache_write_tokens": 1535,
                        "cache_read_tokens": 6395,
                        "output_tokens": 249,
                        "details": {
                            "cache_creation_input_tokens": 1535,
                            "cache_read_input_tokens": 6395,
                            "input_tokens": 3,
                            "output_tokens": 249
                        }
                    }
                },
                {
                    "kind": "response",
                    "model_name": "claude-sonnet-4-5-20250929",
                    "provider_response_id": "msg_bdrk_01AbCdEfGhIjKlMnOpQrStUv",
                    "timestamp": "2026-02-24T08:15:02.000001Z",
                    "usage": {
                        "input_tokens": 12,
                        "output_tokens": 34,
                        "details": {
                            "cache_creation_input_tokens": 0,
                            "cache_read_input_tokens": 0,
                            "input_tokens": 12,
                            "output_tokens": 34
                        }
                    }
                }
            ],
            "usage": {"input_tokens": 7945, "output_tokens": 283}
        })
        .to_string()
    }

    #[test]
    fn maps_modern_response_usage_details_to_token_fields() {
        let fixture = fs_fixture!({
            "sessions/session-a/session_context.json": modern_session_json(),
        });
        let entries =
            read_session_file(&fixture.path("sessions/session-a/session_context.json")).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].session_id, "session-a");
        assert_eq!(entries[0].model, "claude-sonnet-4-5-20250929");
        assert_eq!(
            entries[0].response_id.as_deref(),
            Some("msg_vrtx_014rqTwZTSMNrU4g6HcjDpQp")
        );
        assert_eq!(entries[0].input_tokens, 3);
        assert_eq!(entries[0].output_tokens, 249);
        assert_eq!(entries[0].cache_creation_tokens, 1535);
        assert_eq!(entries[0].cache_read_tokens, 6395);
        assert_eq!(entries[0].timestamp_text, "2026-02-23T21:03:45.664Z");
        assert_eq!(
            entries[0].workspace_path.as_deref(),
            Some("/home/user/project")
        );
    }

    #[test]
    fn maps_legacy_usage_and_falls_back_to_unknown_model() {
        let fixture = fs_fixture!({
            "sessions/session-b/session_context.json": serde_json::json!({
                "id": "session-b",
                "timestamp": 1750592695,
                "workspace_path": "/home/user/legacy",
                "message_history": [
                    {
                        "kind": "response",
                        "model_name": null,
                        "vendor_id": null,
                        "timestamp": "2025-06-22T12:04:55.802259+00:00",
                        "usage": {
                            "requests": 0,
                            "request_tokens": 20807,
                            "response_tokens": 240,
                            "total_tokens": 21047,
                            "details": {
                                "cache_creation_input_tokens": 20803,
                                "cache_read_input_tokens": 0,
                                "input_tokens": 4,
                                "output_tokens": 240
                            }
                        }
                    }
                ]
            })
            .to_string(),
        });
        let entries =
            read_session_file(&fixture.path("sessions/session-b/session_context.json")).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model, DEFAULT_MODEL);
        assert_eq!(entries[0].response_id, None);
        assert_eq!(entries[0].input_tokens, 4);
        assert_eq!(entries[0].output_tokens, 240);
        assert_eq!(entries[0].cache_creation_tokens, 20803);
        assert_eq!(entries[0].cache_read_tokens, 0);
        assert_eq!(entries[0].extra_total_tokens, 0);
        assert_eq!(entries[0].timestamp_text, "2025-06-22T12:04:55.802Z");
    }

    #[test]
    fn derives_uncached_input_from_top_level_when_details_are_missing() {
        let fixture = fs_fixture!({
            "sessions/session-c/session_context.json": serde_json::json!({
                "message_history": [
                    {
                        "kind": "response",
                        "model_name": "claude-sonnet-4-5-20250929",
                        "timestamp": "2026-02-23T21:03:45.664227Z",
                        "usage": {
                            "input_tokens": 7933,
                            "cache_write_tokens": 1535,
                            "cache_read_tokens": 6395,
                            "output_tokens": 249
                        }
                    }
                ]
            })
            .to_string(),
        });
        let entries =
            read_session_file(&fixture.path("sessions/session-c/session_context.json")).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].input_tokens, 3);
        assert_eq!(entries[0].cache_creation_tokens, 1535);
        assert_eq!(entries[0].cache_read_tokens, 6395);
        assert_eq!(entries[0].output_tokens, 249);
    }

    #[test]
    fn skips_requests_zero_token_responses_and_malformed_history_entries() {
        let fixture = fs_fixture!({
            "sessions/session-d/session_context.json": serde_json::json!({
                "message_history": [
                    {"kind": "request", "parts": []},
                    {"kind": "response", "usage": {"input_tokens": 0, "output_tokens": 0}},
                    {"kind": "response"},
                    "not an object",
                    {
                        "kind": "response",
                        "model_name": "claude-sonnet-4-5-20250929",
                        "timestamp": "2026-02-23T21:03:45.664227Z",
                        "usage": {"input_tokens": 5, "output_tokens": 7,
                                   "details": {"input_tokens": 5, "output_tokens": 7,
                                               "cache_creation_input_tokens": 0,
                                               "cache_read_input_tokens": 0}}
                    }
                ]
            })
            .to_string(),
        });
        let entries =
            read_session_file(&fixture.path("sessions/session-d/session_context.json")).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].input_tokens, 5);
    }

    #[test]
    fn falls_back_to_session_epoch_timestamp_when_response_has_none() {
        let fixture = fs_fixture!({
            "sessions/session-e/session_context.json": serde_json::json!({
                "timestamp": 1771880550,
                "message_history": [
                    {
                        "kind": "response",
                        "model_name": "claude-sonnet-4-5-20250929",
                        "usage": {"input_tokens": 5, "output_tokens": 7}
                    }
                ]
            })
            .to_string(),
        });
        let entries =
            read_session_file(&fixture.path("sessions/session-e/session_context.json")).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].timestamp,
            TimestampMs::from_millis(1771880550000)
        );
    }

    #[test]
    fn prices_rovo_usage_from_pricing_map_and_reports_missing_models() {
        let mut pricing = PricingMap::default();
        pricing.load_json(
            r#"{"claude-sonnet-4-5-20250929": {
                "input_cost_per_token": 0.000003,
                "output_cost_per_token": 0.000015,
                "cache_creation_input_token_cost": 0.00000375,
                "cache_read_input_token_cost": 0.0000003
            }}"#,
        );
        let usage = TokenUsageRaw {
            input_tokens: 100,
            output_tokens: 200,
            cache_creation_input_tokens: 300,
            cache_read_input_tokens: 400,
            speed: None,
            cache_creation: None,
        };
        let priced = RovoUsageEntry {
            timestamp: TimestampMs::from_millis(1_771_880_550_000),
            timestamp_text: "2026-02-23T21:02:30.000Z".to_string(),
            session_id: "session-a".to_string(),
            workspace_path: None,
            model: "claude-sonnet-4-5-20250929".to_string(),
            response_id: Some("msg_vrtx_1".to_string()),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_creation_tokens: usage.cache_creation_input_tokens,
            cache_read_tokens: usage.cache_read_input_tokens,
            extra_total_tokens: 0,
        };
        let unknown = RovoUsageEntry {
            model: DEFAULT_MODEL.to_string(),
            ..priced.clone()
        };

        let cost = calculate_rovo_cost(&priced, CostMode::Calculate, &pricing, usage);
        let expected = 100.0 * 0.000003 + 200.0 * 0.000015 + 300.0 * 0.00000375 + 400.0 * 0.0000003;
        assert!((cost - expected).abs() < 1e-12);
        assert_eq!(
            missing_rovo_pricing(&priced, CostMode::Calculate, &pricing, usage),
            None
        );

        assert_eq!(
            calculate_rovo_cost(&unknown, CostMode::Display, &pricing, usage),
            0.0
        );
        assert_eq!(
            missing_rovo_pricing(&unknown, CostMode::Display, &pricing, usage),
            None
        );
        assert!(missing_rovo_pricing(&unknown, CostMode::Calculate, &pricing, usage).is_some());
    }
}
