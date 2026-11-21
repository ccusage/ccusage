use ccusage_core::cost::PriceTable;
use std::sync::{Arc, LazyLock};

use jiff::tz::TimeZone as JiffTimeZone;
use serde::{
    Deserialize, Deserializer,
    de::{self, IgnoredAny, MapAccess, Visitor},
};
use std::fmt;

use crate::{
    LoadedEntry, TokenUsageRaw, apply_total_token_fallback,
    cli::CostMode,
    cost::{EntryPricing, UsageGranularity},
    date_at,
    pricing::MatchKind,
    stored_cost_for_mode,
};
use ccusage_adapter_common::{
    billing::{host_provider, price_entry_for_provider},
    json::JsonText,
    jsonl,
};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct OpenCodeMessage {
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

#[derive(Debug, Default, Deserialize)]
struct OpenCodeCache {
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    read: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    write: u64,
}

#[derive(Debug, Default, Deserialize)]
struct OpenCodeTime {
    #[serde(default, deserialize_with = "jsonl::lenient_i64")]
    created: Option<i64>,
}

struct TimeKey(bool);

impl<'de> Deserialize<'de> for TimeKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct KeyVisitor;

        impl Visitor<'_> for KeyVisitor {
            type Value = TimeKey;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object key")
            }

            fn visit_str<E: de::Error>(self, key: &str) -> Result<Self::Value, E> {
                Ok(TimeKey(key == "time"))
            }
        }

        deserializer.deserialize_str(KeyVisitor)
    }
}

#[derive(Deserialize)]
#[serde(transparent)]
struct LenientTime(#[serde(deserialize_with = "jsonl::lenient_object")] Option<OpenCodeTime>);

struct TimeField<'a>(&'a mut Option<OpenCodeTime>);

impl<'de> Visitor<'de> for TimeField<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        while let Some(TimeKey(is_time)) = map.next_key()? {
            if is_time {
                *self.0 = map.next_value::<LenientTime>()?.0;
                return Ok(());
            }
            map.next_value::<IgnoredAny>()?;
        }
        Ok(())
    }
}

pub(crate) fn message_time_created(data: &[u8]) -> Option<i64> {
    let mut time = None;
    // NOTE(2026-11-19, #2242): Reading stops at the payload's top-level time, so serde_json reports the unread rest as an error. Whether the record is valid stays the typed reader's decision.
    let _ = JsonText::new(data).parse_with(|text| {
        time = None;
        serde_json::Deserializer::from_str(text).deserialize_map(TimeField(&mut time))
    });
    time?.created.filter(|millis| *millis > 0)
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

type OpenCodeNativeLinks = jsonl::FirstField<
    ContentField,
    jsonl::LenientSeq<
        jsonl::FirstField<NativeField, jsonl::FirstField<SessionIdField, jsonl::LenientString>>,
    >,
>;

// NOTE(2026-11-01, #2007): OpenCode claude-agent stores the Claude transcript's session ID in content[].native.sessionID. Use that ID to match usage across the two sources.
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
        let recorded_cost = ccusage_core::cost::recorded_cost(self.stored_cost);
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
        let (pricing_timestamp, granularity) = match self.kind {
            UsageKind::Message => (Some(timestamp), UsageGranularity::Request),
            UsageKind::Aggregate => (None, UsageGranularity::Aggregate),
        };
        let candidates = open_code_model_candidates(&model, &provider);
        let candidates: Vec<_> = candidates
            .iter()
            .map(|(name, kind)| (name.as_str(), *kind))
            .collect();
        let entry_pricing = EntryPricing {
            granularity,
            usage,
            reasoning_tokens: 0,
            residual_tokens: extra_total_tokens,
            reported_model: Some(&model),
            candidates: &candidates,
            stored_cost: recorded_cost,
            timestamp: pricing_timestamp,
        };
        let priced = price_entry_for_provider(entry_pricing, Some(provider.as_str()), pricing);
        let missing_pricing_model = priced.missing_pricing.then(|| model.clone());
        let stored_cost = stored_cost_for_mode(recorded_cost, mode);
        let loaded_session_id = self.session_id.unwrap_or_else(|| "unknown".to_string());
        Ok(Some(LoadedEntry {
            date: date_at(timestamp, tz),
            timestamp,
            project: Arc::clone(&PROJECT),
            session_id: Arc::from(loaded_session_id),
            project_path: Arc::clone(&PROJECT_PATH),
            cost: priced.cost,
            extra_total_tokens,
            credits: None,
            message_count: None,
            model: Some(model),
            usage_limit_reset_time: None,
            missing_pricing_model,
            stored_cost,
            usage,
            message_id: self.id,
            version: None,
            recorded_cost: self.stored_cost,
        }))
    }
}

static PROJECT: LazyLock<Arc<str>> = LazyLock::new(|| Arc::from("opencode"));
static PROJECT_PATH: LazyLock<Arc<str>> = LazyLock::new(|| Arc::from("OpenCode"));

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

pub(crate) fn message_value_to_entry(
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
    // NOTE(2026-11-09, #2050): OpenCode failed compactions can record billed usage without the model. Require a completed compaction before attributing the usage.
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

fn open_code_model_candidates(model: &str, provider: &str) -> Vec<(String, MatchKind)> {
    let resolved = resolve_open_code_model_name(model);
    let normalized = normalize_open_code_model_name(&resolved);
    let normalized = (normalized != resolved && normalized != model).then_some(normalized);
    let kind = if resolved != model || normalized.is_some() {
        MatchKind::Exact
    } else {
        MatchKind::Fuzzy
    };
    let mut base = vec![(model.to_string(), kind)];
    base.extend((resolved != model).then_some((resolved, MatchKind::Fuzzy)));
    base.extend(normalized.map(|name| (name, MatchKind::Fuzzy)));
    let mut candidates = base.clone();
    if provider != "unknown" {
        let provider =
            host_provider(provider).map_or_else(|| provider.replace('-', "_"), str::to_owned);
        candidates.extend(
            base.into_iter()
                .map(|(model, kind)| (format!("{provider}/{model}"), kind)),
        );
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
mod tests;
