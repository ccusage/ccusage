use ccusage_core::cost::PriceTable;
use std::sync::Arc;

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;

use crate::{
    LoadedEntry, TokenUsageRaw, UsageEntry, UsageMessage, apply_total_token_fallback,
    cli::CostMode,
    cost::{EntryPricing, price_entry},
    format_date_tz,
    pricing::MatchKind,
    stored_cost_for_mode,
};
use ccusage_adapter_common::jsonl;

/// A single parsed OpenCode message. Only the fields ccusage consumes are
/// declared; serde skips everything else.
#[derive(Debug, Default, Deserialize)]
pub struct OpenCodeMessage {
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    tokens: Option<OpenCodeTokens>,
    #[serde(
        rename = "modelID",
        default,
        deserialize_with = "jsonl::non_empty_string"
    )]
    model_id: Option<String>,
    #[serde(
        rename = "providerID",
        default,
        deserialize_with = "jsonl::non_empty_string"
    )]
    provider_id: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    time: Option<OpenCodeTime>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    id: Option<String>,
    #[serde(
        rename = "sessionID",
        default,
        deserialize_with = "jsonl::non_empty_string"
    )]
    session_id: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_f64")]
    cost: Option<f64>,
}

/// Token usage block carried by OpenCode messages.
#[derive(Debug, Default, Deserialize)]
struct OpenCodeTokens {
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    input: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    output: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    reasoning: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    cache: Option<OpenCodeCache>,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    total: u64,
}

/// Cache read/write counts nested under OpenCode token usage.
#[derive(Debug, Default, Deserialize)]
struct OpenCodeCache {
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    read: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    write: u64,
}

/// Creation timestamp block carried by OpenCode messages.
#[derive(Debug, Default, Deserialize)]
struct OpenCodeTime {
    #[serde(default, deserialize_with = "jsonl::lenient_i64")]
    created: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct OpenCodeModelReference {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    id: Option<String>,
    #[serde(
        rename = "modelID",
        default,
        deserialize_with = "jsonl::non_empty_string"
    )]
    model_id: Option<String>,
    #[serde(
        rename = "providerID",
        default,
        deserialize_with = "jsonl::non_empty_string"
    )]
    provider_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct OpenCodeV2Message {
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    model: Option<OpenCodeModelReference>,
    #[serde(
        rename = "modelID",
        default,
        deserialize_with = "jsonl::non_empty_string"
    )]
    model_id: Option<String>,
    #[serde(
        rename = "providerID",
        default,
        deserialize_with = "jsonl::non_empty_string"
    )]
    provider_id: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    tokens: Option<OpenCodeTokens>,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    time: Option<OpenCodeTime>,
    #[serde(default, deserialize_with = "jsonl::lenient_f64")]
    cost: Option<f64>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    status: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionMessageKind {
    Assistant,
    /// A compaction request, billed apart from assistant steps.
    Compaction,
}

impl SessionMessageKind {
    pub(crate) fn from_row_type(row_type: &str) -> Option<Self> {
        match row_type {
            "assistant" => Some(Self::Assistant),
            "compaction" => Some(Self::Compaction),
            _ => None,
        }
    }
}

struct ContentField;
impl jsonl::FieldName for ContentField {
    const NAME: &'static str = "content";
}

struct NativeField;
impl jsonl::FieldName for NativeField {
    const NAME: &'static str = "native";
}

struct SessionIdField;
impl jsonl::FieldName for SessionIdField {
    const NAME: &'static str = "sessionID";
}

/// Reads only `content[].native.sessionID`. Every other field, such as message
/// text, is skipped without being built, and a repeated field or a part,
/// `native`, or `sessionID` of another shape reads as absent, so one malformed
/// part cannot unlink the row.
type OpenCodeNativeLinks = jsonl::FirstField<
    ContentField,
    jsonl::LenientSeq<
        jsonl::FirstField<NativeField, jsonl::FirstField<SessionIdField, jsonl::LenientString>>,
    >,
>;

/// The Claude Code session a `claude-agent` row ran in, which names the
/// Claude transcript holding the same turns.
fn native_session_id(data: &str) -> Option<Arc<str>> {
    serde_json::from_str::<OpenCodeNativeLinks>(data)
        .ok()?
        .0?
        .0
        .into_iter()
        .filter_map(|part| part.0?.0?.0)
        .map(|session_id| session_id.trim().to_owned())
        .find(|session_id| !session_id.is_empty())
        .map(Arc::from)
}

impl OpenCodeV2Message {
    fn provider(&self) -> Option<&str> {
        self.model
            .as_ref()
            .and_then(|model| model.provider_id.as_deref())
            .or(self.provider_id.as_deref())
    }

    fn into_usage(
        self,
        id: String,
        session_id: String,
        created: Option<i64>,
    ) -> Option<OpenCodeUsage> {
        let model = self
            .model
            .as_ref()
            .and_then(|model| model.id.clone().or_else(|| model.model_id.clone()))
            .or(self.model_id);
        let provider = self
            .model
            .and_then(|model| model.provider_id)
            .or(self.provider_id);
        Some(OpenCodeUsage {
            id: Some(id),
            session_id: Some(session_id),
            created: self.time.and_then(|time| time.created).or(created),
            model,
            provider,
            tokens: self.tokens?,
            stored_cost: self.cost,
            kind: UsageKind::Message,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UsageKind {
    Message,
    Aggregate,
}

struct OpenCodeUsage {
    id: Option<String>,
    session_id: Option<String>,
    created: Option<i64>,
    model: Option<String>,
    provider: Option<String>,
    tokens: OpenCodeTokens,
    stored_cost: Option<f64>,
    kind: UsageKind,
}

impl OpenCodeUsage {
    fn from_message(
        value: OpenCodeMessage,
        id: Option<String>,
        session_id: Option<String>,
    ) -> Option<Self> {
        Some(Self {
            id: id.or(value.id),
            session_id: session_id.or(value.session_id),
            created: value.time.and_then(|time| time.created),
            model: value.model_id,
            provider: value.provider_id,
            tokens: value.tokens?,
            stored_cost: value.cost,
            kind: UsageKind::Message,
        })
    }

    fn from_aggregate(aggregate: OpenCodeSessionAggregate) -> Self {
        Self {
            id: Some(format!("session:{}", aggregate.session_id)),
            session_id: Some(aggregate.session_id),
            created: aggregate.created,
            model: Some(aggregate.model),
            provider: Some(aggregate.provider),
            tokens: OpenCodeTokens {
                input: aggregate.input_tokens,
                output: aggregate.output_tokens,
                reasoning: aggregate.reasoning_tokens,
                cache: Some(OpenCodeCache {
                    read: aggregate.cache_read_tokens,
                    write: aggregate.cache_write_tokens,
                }),
                total: 0,
            },
            stored_cost: aggregate.cost,
            kind: UsageKind::Aggregate,
        }
    }

    fn into_entry(
        self,
        tz: &JiffTimeZone,
        mode: CostMode,
        pricing: PriceTable<'_>,
    ) -> Result<Option<LoadedEntry>, MissingTimestamp> {
        let cache = self.tokens.cache.as_ref();
        let usage = TokenUsageRaw {
            input_tokens: self.tokens.input,
            output_tokens: self.tokens.output,
            cache_creation_input_tokens: cache.map_or(0, |cache| cache.write),
            cache_read_input_tokens: cache.map_or(0, |cache| cache.read),
            speed: None,
            cache_creation: None,
        };
        let (usage, extra_total_tokens) =
            apply_total_token_fallback(usage, self.tokens.reasoning, self.tokens.total);
        // NOTE(2026-11-17): OpenCode records zero when its model price is unknown.
        let recorded_cost = self.stored_cost.filter(|cost| *cost > 0.0);
        let cost_only = self.kind == UsageKind::Aggregate && recorded_cost.is_some();
        if usage.input_tokens == 0
            && usage.output_tokens == 0
            && usage.cache_creation_input_tokens == 0
            && usage.cache_read_input_tokens == 0
            && extra_total_tokens == 0
            && !cost_only
        {
            return Ok(None);
        }
        let (Some(model), Some(provider)) = (self.model, self.provider) else {
            return Ok(None);
        };
        let timestamp = self
            .created
            .filter(|millis| *millis > 0)
            .ok_or(MissingTimestamp)?;
        let timestamp = crate::TimestampMs::from_millis(timestamp);
        let pricing_timestamp = match self.kind {
            UsageKind::Message => Some(timestamp),
            UsageKind::Aggregate => None,
        };
        let data = UsageEntry {
            session_id: self.session_id,
            timestamp: crate::format_rfc3339_millis(timestamp),
            version: None,
            message: UsageMessage {
                usage,
                model: Some(model.clone()),
                id: self.id,
            },
            cost_usd: self.stored_cost,
            request_id: None,
            is_api_error_message: None,
            is_sidechain: None,
        };
        let candidates = open_code_model_candidates(&model, &provider);
        let candidates: Vec<_> = candidates
            .iter()
            .map(|candidate| (candidate.as_str(), MatchKind::Fuzzy))
            .collect();
        let priced = price_entry(
            EntryPricing {
                usage,
                reasoning_tokens: 0,
                residual_tokens: extra_total_tokens,
                reported_model: Some(&model),
                candidates: &candidates,
                stored_cost: recorded_cost,
                timestamp: pricing_timestamp,
            },
            pricing,
        );
        let missing_pricing_model = priced.missing_pricing.then(|| model.clone());
        let stored_cost = stored_cost_for_mode(recorded_cost, mode);
        let loaded_session_id = data
            .session_id
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        Ok(Some(LoadedEntry {
            date: format_date_tz(timestamp, tz),
            timestamp,
            project: Arc::from("opencode"),
            session_id: Arc::from(loaded_session_id),
            project_path: Arc::from("OpenCode"),
            cost: priced.cost,
            extra_total_tokens,
            credits: None,
            message_count: None,
            model: Some(model),
            usage_limit_reset_time: None,
            missing_pricing_model,
            stored_cost,
            data,
        }))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct MissingTimestamp;

impl MissingTimestamp {
    pub(crate) const REASON: &str = "OpenCode usage record has no creation time";
}

#[derive(Debug)]
pub(crate) enum SessionMessageError {
    Decode(serde_json::Error),
    MissingTimestamp,
}

pub fn message_value_to_entry(
    value: OpenCodeMessage,
    id: Option<String>,
    session_id: Option<String>,
    tz: &JiffTimeZone,
    mode: CostMode,
    pricing: PriceTable<'_>,
) -> Result<Option<LoadedEntry>, MissingTimestamp> {
    OpenCodeUsage::from_message(value, id, session_id)
        .map_or(Ok(None), |usage| usage.into_entry(tz, mode, pricing))
}

pub(crate) struct SessionMessageRow<'a> {
    pub(crate) kind: SessionMessageKind,
    pub(crate) data: &'a str,
    pub(crate) id: String,
    pub(crate) session_id: String,
    pub(crate) created: Option<i64>,
}

pub(crate) fn session_message_value_to_entry_checked(
    row: SessionMessageRow<'_>,
    tz: &JiffTimeZone,
    mode: CostMode,
    pricing: PriceTable<'_>,
) -> Result<Option<crate::LinkedEntry>, SessionMessageError> {
    let SessionMessageRow {
        kind,
        data,
        id,
        session_id,
        created,
    } = row;
    let value =
        serde_json::from_str::<OpenCodeV2Message>(data).map_err(SessionMessageError::Decode)?;
    // Running and failed compactions are out of scope: a failed one records
    // the usage it was billed but no model to attribute it to.
    if kind == SessionMessageKind::Compaction && value.status.as_deref() != Some("completed") {
        return Ok(None);
    }
    let native_session = (value.provider() == Some("claude-agent"))
        .then(|| native_session_id(data))
        .flatten();
    let Some(usage) = value.into_usage(id, session_id, created) else {
        return Ok(None);
    };
    let entry = usage
        .into_entry(tz, mode, pricing)
        .map_err(|MissingTimestamp| SessionMessageError::MissingTimestamp)?;
    Ok(entry.map(|entry| (entry, native_session)))
}

pub(crate) struct OpenCodeSessionAggregate {
    pub(crate) session_id: String,
    pub(crate) created: Option<i64>,
    pub(crate) model: String,
    pub(crate) provider: String,
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) reasoning_tokens: u64,
    pub(crate) cache_read_tokens: u64,
    pub(crate) cache_write_tokens: u64,
    pub(crate) cost: Option<f64>,
}

pub(crate) fn session_value_to_entry(
    aggregate: OpenCodeSessionAggregate,
    tz: &JiffTimeZone,
    mode: CostMode,
    pricing: PriceTable<'_>,
) -> Result<Option<LoadedEntry>, MissingTimestamp> {
    OpenCodeUsage::from_aggregate(aggregate).into_entry(tz, mode, pricing)
}

fn open_code_model_candidates(model: &str, provider: &str) -> Vec<String> {
    let resolved = resolve_open_code_model_name(model);
    let normalized = normalize_open_code_model_name(&resolved);
    let mut base = vec![resolved];
    if normalized != base[0] {
        base.push(normalized);
    }
    let mut candidates = base.clone();
    if provider != "unknown" {
        let provider = provider.replace('-', "_");
        candidates.extend(base.into_iter().map(|model| format!("{provider}/{model}")));
    }
    candidates.dedup();
    candidates
}

fn resolve_open_code_model_name(model: &str) -> String {
    match model {
        "gemini-3-pro-high" => "gemini-3-pro-preview".to_string(),
        "k2p6" => "kimi-k2.6".to_string(),
        _ => model.to_string(),
    }
}

fn normalize_open_code_model_name(model: &str) -> String {
    for family in ["claude-haiku-", "claude-opus-", "claude-sonnet-"] {
        if let Some(rest) = model.strip_prefix(family) {
            if let Some((major, minor_and_suffix)) = rest.split_once('.')
                && major.chars().all(|ch| ch.is_ascii_digit())
                && minor_and_suffix
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_digit())
            {
                return format!("{family}{major}-{minor_and_suffix}");
            }
            let mut chars = rest.chars();
            if let (Some(major), Some(minor)) = (chars.next(), chars.next())
                && major.is_ascii_digit()
                && minor.is_ascii_digit()
            {
                return format!("{family}{major}-{minor}{}", chars.collect::<String>());
            }
        }
    }
    model.to_string()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        MissingTimestamp, OpenCodeMessage, OpenCodeSessionAggregate, SessionMessageKind,
        SessionMessageRow, message_value_to_entry, open_code_model_candidates,
        session_message_value_to_entry_checked, session_value_to_entry,
    };
    use crate::{LoadedEntry, PricingMap, cli::CostMode};

    fn v2_native_session(data: &str) -> Option<String> {
        session_message_value_to_entry_checked(
            SessionMessageRow {
                kind: SessionMessageKind::Assistant,
                data,
                id: "msg-v2".to_string(),
                session_id: "v2-session".to_string(),
                created: Some(1_767_312_000_000),
            },
            &jiff::tz::TimeZone::system(),
            CostMode::Display,
            ccusage_core::cost::PriceTable::Display,
        )
        .unwrap()
        .unwrap()
        .1
        .map(|session| session.to_string())
    }

    #[test]
    fn links_claude_agent_rows_to_their_claude_session() {
        assert_eq!(
            v2_native_session(
                r#"{"model":{"id":"opus","providerID":"claude-agent"},"tokens":{"input":100},"content":[{"type":"reasoning","text":"…"},{"type":"text","native":{"sessionID":"claude-session-1","messageID":"m"}}]}"#
            )
            .as_deref(),
            Some("claude-session-1")
        );
    }

    #[test]
    fn ignores_native_links_from_other_providers() {
        assert_eq!(
            v2_native_session(
                r#"{"model":{"id":"gpt-test","providerID":"openai"},"tokens":{"input":100},"content":[{"native":{"sessionID":"other-session"}}]}"#
            ),
            None
        );
    }

    #[test]
    fn leaves_claude_agent_rows_without_a_native_link_unlinked() {
        assert_eq!(
            v2_native_session(
                r#"{"model":{"id":"opus","providerID":"claude-agent"},"tokens":{"input":100},"content":[{"type":"text","native":{"sessionID":""}},"not a part"]}"#
            ),
            None
        );
    }

    #[test]
    fn keeps_the_native_link_when_other_parts_are_malformed() {
        for content in [
            r#"[{"native":{"sessionID":"s"}},"junk"]"#,
            r#"[{"native":5},{"native":{"sessionID":7}},[1,{"a":2}],null,{"native":{"sessionID":"s"}}]"#,
            r#"[{"native":{"sessionID":{"x":1}}},{"native":{"sessionID":["y"]}},{"native":{"sessionID":"s"}}]"#,
            r#"[{"native":{"sessionID":"s"}},{"native":{"sessionID":{"x":1}}},{"native":{"sessionID":["y"]}}]"#,
            r#"[{"native":{"sessionID":"s","sessionID":"t"}}]"#,
            r#"[{"native":{"sessionID":5,"sessionID":"s"}}]"#,
            r#"[{"native":7,"native":{"sessionID":"s"},"native":{"sessionID":"t"}}]"#,
        ] {
            assert_eq!(
                v2_native_session(&format!(
                    r#"{{"model":{{"id":"opus","providerID":"claude-agent"}},"tokens":{{"input":100}},"content":{content}}}"#
                ))
                .as_deref(),
                Some("s"),
                "{content}"
            );
        }
    }

    #[test]
    fn skips_blank_native_session_ids() {
        assert_eq!(
            v2_native_session(
                r#"{"model":{"id":"opus","providerID":"claude-agent"},"tokens":{"input":100},"content":[{"type":"text","native":{"sessionID":" "}},{"type":"tool","native":{"sessionID":"claude-session-2","resultState":{"x":[1]}}}]}"#
            )
            .as_deref(),
            Some("claude-session-2")
        );
    }

    fn message(value: serde_json::Value) -> OpenCodeMessage {
        serde_json::from_value(value).unwrap()
    }

    fn entry_snapshot(entry: &LoadedEntry) -> serde_json::Value {
        json!({
            "date": entry.date,
            "timestamp": entry.timestamp.as_millis(),
            "sessionId": entry.session_id.as_ref(),
            "project": entry.project.as_ref(),
            "projectPath": entry.project_path.as_ref(),
            "cost": entry.cost,
            "extraTotalTokens": entry.extra_total_tokens,
            "model": entry.model.as_deref(),
            "data": {
                "sessionId": entry.data.session_id.as_deref(),
                "timestamp": entry.data.timestamp,
                "version": entry.data.version.as_deref(),
                "message": {
                    "id": entry.data.message.id.as_deref(),
                    "model": entry.data.message.model.as_deref(),
                    "usage": {
                        "inputTokens": entry.data.message.usage.input_tokens,
                        "outputTokens": entry.data.message.usage.output_tokens,
                        "cacheCreationInputTokens": entry.data.message.usage.cache_creation_input_tokens,
                        "cacheReadInputTokens": entry.data.message.usage.cache_read_input_tokens,
                    },
                },
                "costUSD": entry.data.cost_usd,
            },
        })
    }

    #[test]
    fn calculates_cost_when_opencode_stores_zero_cost() {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(
            r#"{
                "gpt-test": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000010,
                    "cache_read_input_token_cost": 0.0000001
                }
            }"#,
        );
        let entry = message_value_to_entry(
            message(json!({
                "id": "message-a",
                "sessionID": "session-a",
                "providerID": "openai",
                "modelID": "gpt-test",
                "time": { "created": 1767312000000i64 },
                "tokens": {
                    "input": 100,
                    "output": 10,
                    "cache": { "read": 50 }
                },
                "cost": 0
            })),
            None,
            None,
            &jiff::tz::TimeZone::system(),
            CostMode::Auto,
            ccusage_core::cost::PriceTable::Auto(&pricing),
        )
        .unwrap()
        .unwrap();

        assert_eq!(entry.cost, 0.000205);
    }

    #[test]
    fn resolves_a_provider_identity_before_fuzzy_matches_for_both_token_sides() {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(r#"{
            "azure/us-gov/text-embedding-3-small": {"input_cost_per_token": 0.025, "output_cost_per_token": 0},
            "ambient/small": {"input_cost_per_token": 0.001, "output_cost_per_token": 0.001}
        }"#);
        for (case, tokens) in [
            ("73_input_only", json!({"input": 100})),
            ("74_output_only", json!({"output": 100})),
        ] {
            let entry = message_value_to_entry(
                message(json!({
                    "id": "message-a",
                    "sessionID": "session-a",
                    "providerID": "ambient",
                    "modelID": "small",
                    "time": {"created": 1767312000000i64},
                    "tokens": tokens
                })),
                None,
                None,
                &jiff::tz::TimeZone::system(),
                CostMode::Calculate,
                ccusage_core::cost::PriceTable::Calculate(&pricing),
            )
            .unwrap()
            .unwrap();
            assert_eq!(entry.cost, 0.1, "{case}");
            assert_eq!(entry.missing_pricing_model, None, "{case}");
        }
    }

    #[test]
    fn keeps_the_same_model_when_its_output_rate_is_zero() {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(
            r#"{
            "small": {"input_cost_per_token": 0.025, "output_cost_per_token": 0},
            "ambient/small": {"input_cost_per_token": 0.001, "output_cost_per_token": 0.001}
        }"#,
        );
        let entry = message_value_to_entry(
            message(json!({
                "id": "message-a",
                "sessionID": "session-a",
                "providerID": "ambient",
                "modelID": "small",
                "time": {"created": 1767312000000i64},
                "tokens": {"output": 100}
            })),
            None,
            None,
            &jiff::tz::TimeZone::system(),
            CostMode::Calculate,
            ccusage_core::cost::PriceTable::Calculate(&pricing),
        )
        .unwrap()
        .unwrap();
        assert_eq!(entry.cost, 0.0);
        assert_eq!(entry.missing_pricing_model, None);
    }

    #[test]
    fn keeps_positive_opencode_cost() {
        let entry = message_value_to_entry(
            message(json!({
                "id": "message-a",
                "sessionID": "session-a",
                "providerID": "openai",
                "modelID": "gpt-test",
                "time": { "created": 1767312000000i64 },
                "tokens": {
                    "input": 100
                },
                "cost": 0.02
            })),
            None,
            None,
            &jiff::tz::TimeZone::system(),
            CostMode::Auto,
            ccusage_core::cost::PriceTable::for_map(
                CostMode::Auto,
                &ccusage_core::PricingMap::default(),
            ),
        )
        .unwrap()
        .unwrap();

        assert_eq!(entry.cost, 0.02);
    }

    #[test]
    fn honors_cost_modes_for_recorded_and_unrecorded_costs() {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(
            r#"{"gpt-test":{"input_cost_per_token":0.001,"output_cost_per_token":0.01}}"#,
        );
        for (mode, recorded, expected, stored) in [
            (CostMode::Calculate, Some(9.0), 0.2, None),
            (CostMode::Display, None, 0.0, None),
            (CostMode::Display, Some(0.0), 0.0, None),
            (CostMode::Display, Some(9.0), 9.0, None),
            (CostMode::Auto, Some(9.0), 9.0, Some(9.0)),
            (CostMode::Auto, Some(0.0), 0.2, None),
            (CostMode::Auto, Some(-1.0), 0.2, None),
            (CostMode::Auto, None, 0.2, None),
            (CostMode::Calculate, None, 0.2, None),
        ] {
            let entry = message_value_to_entry(
                message(json!({
                    "providerID": "openai", "modelID": "gpt-test",
                    "time": { "created": 1767312000000i64 },
                    "tokens": { "input": 100, "output": 10 }, "cost": recorded
                })),
                None,
                None,
                &jiff::tz::TimeZone::UTC,
                mode,
                ccusage_core::cost::PriceTable::for_map(mode, &pricing),
            )
            .unwrap()
            .unwrap();
            assert_eq!(entry.cost, expected, "{mode:?}, {recorded:?}");
            assert_eq!(entry.stored_cost, stored, "{mode:?}, {recorded:?}");
            assert_eq!(entry.missing_pricing_model, None);
        }
    }

    #[test]
    fn keeps_a_free_first_candidate_instead_of_a_paid_provider_candidate() {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(
            r#"{
                "gpt-test":{"litellm_provider": "openai", "input_cost_per_token":0,"output_cost_per_token":0},
                "openai/gpt-test":{"input_cost_per_token":1,"output_cost_per_token":2}
            }"#,
        );
        let entry = message_value_to_entry(
            message(json!({
                "providerID": "openai", "modelID": "gpt-test",
                "time": { "created": 1767312000000i64 },
                "tokens": { "input": 100, "output": 10 }, "cost": 0
            })),
            None,
            None,
            &jiff::tz::TimeZone::UTC,
            CostMode::Auto,
            ccusage_core::cost::PriceTable::Auto(&pricing),
        )
        .unwrap()
        .unwrap();
        assert_eq!(entry.cost, 0.0);
        assert_eq!(entry.missing_pricing_model, None);
    }

    #[test]
    fn reports_missing_calculated_prices_even_with_a_recorded_cost() {
        let pricing = ccusage_core::PricingMap::default();
        for (mode, expected_cost, missing) in [
            (CostMode::Calculate, 0.0, Some("opencode-unpriced-model")),
            (CostMode::Auto, 9.0, None),
            (CostMode::Display, 9.0, None),
        ] {
            let entry = message_value_to_entry(
                message(json!({
                    "providerID": "openai", "modelID": "opencode-unpriced-model",
                    "time": { "created": 1767312000000i64 },
                    "tokens": { "input": 100 }, "cost": 9
                })),
                None,
                None,
                &jiff::tz::TimeZone::UTC,
                mode,
                ccusage_core::cost::PriceTable::for_map(mode, &pricing),
            )
            .unwrap()
            .unwrap();
            assert_eq!(entry.cost, expected_cost, "{mode:?}");
            assert_eq!(entry.missing_pricing_model.as_deref(), missing, "{mode:?}");
        }
    }

    #[test]
    fn primary_rates_outrank_models_dev_for_qualified_candidates() {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = ccusage_core::PricingMap::load_embedded();
        pricing.load_json(
            r#"{"claude-sonnet-4":{"litellm_provider":"anthropic","input_cost_per_token":0.125,"output_cost_per_token":0.25}}"#,
        );
        for model in [
            "anthropic/claude-sonnet-4",
            "openrouter/anthropic/claude-sonnet-4",
        ] {
            let entry = message_value_to_entry(
                message(json!({
                    "providerID": "openrouter", "modelID": model,
                    "time": { "created": 1767312000000i64 },
                    "tokens": { "input": 100, "output": 10 }, "cost": 0
                })),
                None,
                None,
                &jiff::tz::TimeZone::UTC,
                CostMode::Calculate,
                ccusage_core::cost::PriceTable::Calculate(&pricing),
            )
            .unwrap()
            .unwrap();
            assert_eq!(entry.cost, 15.0, "{model}");
        }
    }

    #[test]
    fn preserves_existing_reasoning_and_residual_billing() {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(
            r#"{"gpt-test":{"input_cost_per_token":0.125,"output_cost_per_token":0.25}}"#,
        );
        let entry = message_value_to_entry(
            message(json!({
                "providerID": "openai", "modelID": "gpt-test",
                "time": { "created": 1767312000000i64 },
                "tokens": { "input": 8, "output": 4, "reasoning": 2, "total": 20 }
            })),
            None,
            None,
            &jiff::tz::TimeZone::UTC,
            CostMode::Calculate,
            ccusage_core::cost::PriceTable::Calculate(&pricing),
        )
        .unwrap()
        .unwrap();
        assert_eq!(entry.data.message.usage.output_tokens, 4);
        assert_eq!(entry.extra_total_tokens, 8);
        assert_eq!(entry.cost, 4.0);
    }

    #[test]
    fn keeps_opencode_record_when_cache_field_is_not_an_object() {
        let entry = message_value_to_entry(
            message(json!({
                "id": "message-a",
                "sessionID": "session-a",
                "providerID": "openai",
                "modelID": "gpt-test",
                "time": { "created": 1767312000000i64 },
                "tokens": {
                    "input": 100,
                    "output": 10,
                    "cache": 0
                },
                "cost": 0.02
            })),
            None,
            None,
            &jiff::tz::TimeZone::system(),
            CostMode::Auto,
            ccusage_core::cost::PriceTable::for_map(
                CostMode::Auto,
                &ccusage_core::PricingMap::default(),
            ),
        )
        .unwrap()
        .unwrap();

        assert_eq!(entry.data.message.usage.input_tokens, 100);
        assert_eq!(entry.data.message.usage.output_tokens, 10);
        assert_eq!(entry.data.message.usage.cache_creation_input_tokens, 0);
        assert_eq!(entry.data.message.usage.cache_read_input_tokens, 0);
        assert_eq!(entry.cost, 0.02);
    }

    #[test]
    fn falls_back_to_total_tokens_when_opencode_token_parts_are_missing() {
        let entry = message_value_to_entry(
            message(json!({
                "id": "message-a",
                "sessionID": "session-a",
                "providerID": "openai",
                "modelID": "gpt-test",
                "time": { "created": 1767312000000i64 },
                "tokens": {
                    "total": 123
                }
            })),
            None,
            None,
            &jiff::tz::TimeZone::system(),
            CostMode::Auto,
            ccusage_core::cost::PriceTable::for_map(
                CostMode::Auto,
                &ccusage_core::PricingMap::default(),
            ),
        )
        .unwrap()
        .unwrap();

        assert_eq!(entry.data.message.usage.output_tokens, 123);
        assert_eq!(entry.extra_total_tokens, 0);
    }

    #[test]
    fn creates_open_code_provider_and_normalized_model_candidates() {
        assert_eq!(
            open_code_model_candidates("claude-sonnet-4.5", "github-copilot"),
            vec![
                "claude-sonnet-4.5",
                "claude-sonnet-4-5",
                "github_copilot/claude-sonnet-4.5",
                "github_copilot/claude-sonnet-4-5",
            ]
        );
    }

    #[test]
    fn preserves_legacy_prices_for_provider_qualified_models() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        let models = [
            ("anthropic", "claude-sonnet-4-20250514"),
            ("anthropic", "claude-sonnet-4.5"),
            ("anthropic", "claude-sonnet-45"),
            ("anthropic", "claude-opus-4-20250514"),
            ("anthropic", "claude-haiku-4-5-20251001"),
            ("openai", "gpt-4o"),
            ("openai", "gpt-4.1"),
            ("openai", "gpt-5"),
            ("openai", "gpt-5-mini"),
            ("openai", "o3"),
            ("openai", "o4-mini"),
            ("google", "gemini-2.5-pro"),
            ("google", "gemini-2.5-flash"),
            ("google", "gemini-3-pro-high"),
            ("google", "gemini-3-pro-preview"),
            ("deepseek", "deepseek-chat"),
            ("deepseek", "deepseek-v4-flash"),
            ("xai", "grok-3"),
            ("xai", "grok-4"),
            ("moonshotai", "kimi-k2.6"),
            ("kimi-for-coding", "k2p6"),
            ("github-copilot", "claude-sonnet-4.5"),
            ("qwen", "qwen3-coder"),
            ("unknown", "opencode-unpriced-model"),
        ];
        let usage = crate::TokenUsageRaw {
            input_tokens: 1234,
            output_tokens: 567,
            cache_read_input_tokens: 89,
            cache_creation_input_tokens: 23,
            ..Default::default()
        };
        let mut comparisons = 0;
        let mut priced = 0;
        for (provider, model) in models {
            for (provider, model) in [
                (provider, model.to_string()),
                (provider, format!("{provider}/{model}")),
                ("openrouter", format!("{provider}/{model}")),
                ("openrouter", format!("openrouter/{provider}/{model}")),
            ] {
                for timestamp in ["2026-08-17T01:00:00Z", "2026-08-17T13:00:00Z"] {
                    let timestamp = crate::parse_ts_timestamp(timestamp).unwrap();
                    let old_cost = open_code_model_candidates(&model, provider)
                        .into_iter()
                        .map(|candidate| {
                            ccusage_core::cost::price_reported_usage(
                                Some(&candidate),
                                usage,
                                None,
                                timestamp,
                                ccusage_core::cost::PriceTable::Calculate(&pricing),
                            )
                            .0
                        })
                        .find(|cost| *cost > 0.0)
                        .unwrap_or(0.0);
                    let entry = message_value_to_entry(
                        message(json!({
                            "providerID": provider, "modelID": model,
                            "time": { "created": timestamp.as_millis() },
                            "tokens": { "input": 1234, "output": 567,
                                "cache": { "read": 89, "write": 23 } },
                            "cost": 0
                        })),
                        None,
                        None,
                        &jiff::tz::TimeZone::UTC,
                        CostMode::Auto,
                        ccusage_core::cost::PriceTable::Auto(&pricing),
                    )
                    .unwrap()
                    .unwrap();
                    assert_eq!(entry.cost, old_cost, "{provider}/{model} at {timestamp:?}");
                    comparisons += 1;
                    priced += usize::from(old_cost > 0.0);
                }
            }
        }
        assert_eq!(comparisons, 192);
        assert!(
            priced >= 144,
            "only {priced}/{comparisons} cases were priced"
        );
        eprintln!("Legacy pricing parity: {comparisons} cases, {priced} priced");
    }

    #[test]
    fn calculates_cost_for_k2p6_when_opencode_stores_zero_cost() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing = ccusage_core::PricingMap::load_embedded();
        let entry = message_value_to_entry(
            message(json!({
                "id": "message-a",
                "sessionID": "session-a",
                "providerID": "kimi-for-coding",
                "modelID": "k2p6",
                "time": { "created": 1767312000000i64 },
                "tokens": {
                    "input": 100,
                    "output": 10,
                    "cache": { "read": 50 }
                },
                "cost": 0
            })),
            None,
            None,
            &jiff::tz::TimeZone::system(),
            CostMode::Auto,
            ccusage_core::cost::PriceTable::Auto(&pricing),
        )
        .unwrap()
        .unwrap();

        // The rate comes from LiteLLM's per-token prices or, when models.dev
        // marks the id exact-only, from its per-million prices, and the two
        // round differently in the last bit.
        assert!(
            (entry.cost - 0.000143).abs() < 1e-12,
            "k2p6 cost was {}",
            entry.cost
        );
    }

    #[test]
    fn snapshots_message_to_entry_variants_and_model_candidates() {
        ccusage_test_support::install_pricing_snapshots();
        let tz = crate::resolve_tz(Some("UTC"));
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(
            r#"{
                "github_copilot/claude-sonnet-4-5": {
                    "input_cost_per_token": 0.125,
                    "output_cost_per_token": 0.25,
                    "cache_read_input_token_cost": 0.0625
                }
            }"#,
        );
        let calculated = message_value_to_entry(
            message(json!({
                "id": "message-a",
                "sessionID": "session-a",
                "providerID": "github-copilot",
                "modelID": "claude-sonnet-4.5",
                "time": { "created": 1767312000000i64 },
                "tokens": {
                    "input": 100,
                    "output": 10,
                    "cache": { "read": 50, "write": 25 },
                    "total": 185
                },
                "cost": 0
            })),
            None,
            None,
            &tz,
            CostMode::Auto,
            ccusage_core::cost::PriceTable::Auto(&pricing),
        )
        .unwrap()
        .unwrap();
        let display_cost = message_value_to_entry(
            message(json!({
                "id": "message-b",
                "providerID": "openai",
                "modelID": "gpt-test",
                "time": { "created": 1767312000000i64 },
                "tokens": { "total": 123 },
                "cost": 0.02
            })),
            None,
            Some("explicit-session".to_string()),
            &tz,
            CostMode::Display,
            ccusage_core::cost::PriceTable::Display,
        )
        .unwrap()
        .unwrap();

        insta::assert_json_snapshot!(json!({
            "calculated": entry_snapshot(&calculated),
            "displayCost": entry_snapshot(&display_cost),
            "candidates": {
                "anthropic": open_code_model_candidates("claude-sonnet-4.5", "anthropic"),
                "copilot": open_code_model_candidates("claude-sonnet-4.5", "github-copilot"),
                "geminiAlias": open_code_model_candidates("gemini-3-pro-high", "google"),
                "unknownProvider": open_code_model_candidates("gpt-test", "unknown"),
            }
        }));
    }

    fn deepseek_pricing(input: f64) -> PricingMap {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(&format!(
            "{{\"deepseek-v4-flash\":{{\"litellm_provider\":\"deepseek\",\"input_cost_per_token\":{input},\"output_cost_per_token\":0.00000028}}}}"
        ));
        pricing
    }

    #[test]
    fn rejects_usage_without_a_positive_creation_time() {
        for time in [
            json!(null),
            json!({}),
            json!({ "created": 0 }),
            json!({ "created": -1 }),
        ] {
            assert_eq!(
                message_value_to_entry(
                    message(json!({
                        "id": "message-a",
                        "sessionID": "session-a",
                        "providerID": "openai",
                        "modelID": "gpt-test",
                        "time": time,
                        "tokens": { "input": 10 }
                    })),
                    None,
                    None,
                    &jiff::tz::TimeZone::system(),
                    CostMode::Display,
                    ccusage_core::cost::PriceTable::Display,
                )
                .map(|entry| entry.is_some()),
                Err(MissingTimestamp),
                "{time}"
            );
        }
    }

    #[test]
    fn rejects_session_aggregates_without_a_positive_creation_time() {
        let aggregate = OpenCodeSessionAggregate {
            session_id: "session-a".to_string(),
            created: Some(0),
            model: "gpt-test".to_string(),
            provider: "openai".to_string(),
            input_tokens: 10,
            output_tokens: 0,
            reasoning_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost: None,
        };

        assert_eq!(
            session_value_to_entry(
                aggregate,
                &jiff::tz::TimeZone::system(),
                CostMode::Display,
                ccusage_core::cost::PriceTable::Display
            )
            .map(|entry| entry.is_some()),
            Err(MissingTimestamp)
        );
    }

    #[test]
    fn prices_cumulative_session_aggregates_with_static_rates() {
        let pricing = deepseek_pricing(0.00000014);
        let created = crate::parse_ts_timestamp("2026-08-17T01:00:00Z")
            .unwrap()
            .as_millis();
        let entry = session_value_to_entry(
            OpenCodeSessionAggregate {
                session_id: "session-a".to_string(),
                created: Some(created),
                model: "deepseek-v4-flash".to_string(),
                provider: "deepseek".to_string(),
                input_tokens: 1_000_000,
                output_tokens: 0,
                reasoning_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost: None,
            },
            &jiff::tz::TimeZone::system(),
            CostMode::Calculate,
            ccusage_core::cost::PriceTable::Calculate(&pricing),
        )
        .unwrap()
        .unwrap();

        assert_eq!(entry.timestamp.as_millis(), created);
        assert_eq!(entry.cost, 0.14);
    }
}
