use std::sync::Arc;

use serde::Deserialize;

use crate::{
    GuardedUsage, IteratedTokenUsage, Nullable, Rejection, TimestampMs, TokenUsageRaw,
    advisor::AdvisorLookup,
    cli::IsoDate,
    dedupe::{DedupEntry, DedupKey, Request},
    deserialize_usage_line, non_null,
};

#[derive(Debug)]
pub(super) struct ClaudeEntry {
    pub(super) key: DedupKey,
    pub(super) date: IsoDate,
    pub(super) project: Arc<str>,
    pub(super) usage: TokenUsageRaw,
    pub(super) cost: f64,
    pub(super) model: Option<String>,
    pub(super) missing_pricing_model: Option<String>,
    pub(super) recorded_cost: RecordedCost,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum RecordedCost {
    Absent,
    Unbilled,
    Billed(f64),
}

impl RecordedCost {
    pub(super) fn new(cost_usd: Option<f64>, billed: Option<f64>) -> Self {
        match (cost_usd, billed) {
            (None, _) => Self::Absent,
            (Some(_), None) => Self::Unbilled,
            (Some(_), Some(cost)) => Self::Billed(cost),
        }
    }

    pub(super) fn billed(self) -> Option<f64> {
        match self {
            Self::Billed(cost) => Some(cost),
            Self::Absent | Self::Unbilled => None,
        }
    }
}

impl DedupEntry for ClaudeEntry {
    fn timestamp(&self) -> TimestampMs {
        self.key.timestamp
    }

    fn session_id(&self) -> &str {
        &self.key.session_id
    }

    fn owned_session_id(&self) -> Arc<str> {
        Arc::clone(&self.key.session_id)
    }

    fn message_id(&self) -> Option<&str> {
        self.key.message_id.as_deref()
    }

    fn request(&self) -> &Request {
        &self.key.request
    }

    fn is_sidechain(&self) -> bool {
        self.key.is_sidechain == Some(true)
    }

    fn usage(&self) -> &TokenUsageRaw {
        &self.usage
    }

    fn records_cost(&self) -> bool {
        !matches!(self.recorded_cost, RecordedCost::Absent)
    }
}

#[derive(Debug)]
pub(crate) struct ClaudeUsageRecord {
    pub(super) timestamp: String,
    pub(crate) message: ClaudeUsageMessage,
    pub(super) version: Option<String>,
    pub(super) session_id: Option<String>,
    pub(super) cost_usd: Option<f64>,
    pub(super) request_id: Option<String>,
    pub(super) is_sidechain: Option<bool>,
    pub(super) is_api_error_message: Option<bool>,
}

#[derive(Debug)]
pub(crate) struct ClaudeUsageMessage {
    pub(crate) usage: IteratedTokenUsage,
    pub(super) model: Option<String>,
    pub(super) id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawUsageRecord {
    timestamp: String,
    message: RawUsageMessage,
    #[serde(default, deserialize_with = "crate::nullable")]
    version: Nullable<String>,
    #[serde(default, deserialize_with = "crate::nullable")]
    session_id: Nullable<String>,
    #[serde(rename = "costUSD")]
    #[serde(default, deserialize_with = "crate::nullable")]
    cost_usd: Nullable<f64>,
    #[serde(default, deserialize_with = "crate::nullable")]
    request_id: Nullable<String>,
    is_sidechain: Option<bool>,
    #[serde(default, deserialize_with = "crate::nullable")]
    is_api_error_message: Nullable<bool>,
}

impl RawUsageRecord {
    fn decode(self) -> std::result::Result<ClaudeUsageRecord, Rejection> {
        Ok(ClaudeUsageRecord {
            timestamp: self.timestamp,
            message: self.message.decode()?,
            version: non_null(self.version)?,
            session_id: non_null(self.session_id)?,
            cost_usd: non_null(self.cost_usd)?,
            request_id: non_null(self.request_id)?,
            is_sidechain: self.is_sidechain,
            is_api_error_message: non_null(self.is_api_error_message)?,
        })
    }
}

#[derive(Debug, Deserialize)]
struct RawUsageMessage {
    usage: GuardedUsage,
    #[serde(default, deserialize_with = "crate::nullable")]
    model: Nullable<String>,
    #[serde(default, deserialize_with = "crate::nullable")]
    id: Nullable<String>,
}

impl RawUsageMessage {
    fn decode(self) -> std::result::Result<ClaudeUsageMessage, Rejection> {
        if self.usage.has_null {
            return Err(Rejection::NullContract);
        }
        Ok(ClaudeUsageMessage {
            usage: self.usage.usage,
            model: non_null(self.model)?,
            id: non_null(self.id)?,
        })
    }
}

pub(crate) fn parse_usage_line(
    line: &[u8],
) -> std::result::Result<(ClaudeUsageRecord, AdvisorLookup), Rejection> {
    match deserialize_usage_line::<RawUsageRecord>(line) {
        Ok(raw) => {
            let entry = raw.decode()?;
            let lookup = if entry.message.usage.has_advisor_iteration {
                AdvisorLookup::AskLine
            } else {
                AdvisorLookup::RuledOut
            };
            Ok((entry, lookup))
        }
        Err(_) => deserialize_usage_line::<AgentProgressEntry>(line)
            .and_then(AgentProgressEntry::decode)
            .map(|entry| {
                let lookup = if entry.message.usage.has_advisor_iteration {
                    AdvisorLookup::AskProgress
                } else {
                    AdvisorLookup::RuledOut
                };
                (entry, lookup)
            }),
    }
}

// NOTE(2026-05-19, #977): Claude Code also writes assistant usage under `data.message.message` in AgentProgress records. Read that envelope for subagent usage.
#[derive(Debug, Deserialize)]
struct AgentProgressEntry {
    #[serde(rename = "sessionId")]
    #[serde(default, deserialize_with = "crate::nullable")]
    session_id: Nullable<String>,
    data: AgentProgressData,
}

impl AgentProgressEntry {
    fn decode(self) -> std::result::Result<ClaudeUsageRecord, Rejection> {
        let message = self.data.message;
        Ok(ClaudeUsageRecord {
            timestamp: message.timestamp,
            message: message.message.decode()?,
            version: None,
            is_api_error_message: non_null(message.is_api_error_message)?,
            session_id: non_null(self.session_id)?,
            cost_usd: non_null(message.cost_usd)?,
            request_id: non_null(message.request_id)?,
            is_sidechain: message.is_sidechain,
        })
    }
}

#[derive(Debug, Deserialize)]
struct AgentProgressData {
    message: AgentProgressMessage,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentProgressMessage {
    #[serde(default, deserialize_with = "crate::nullable")]
    is_api_error_message: Nullable<bool>,
    timestamp: String,
    message: RawUsageMessage,
    #[serde(rename = "costUSD")]
    #[serde(default, deserialize_with = "crate::nullable")]
    cost_usd: Nullable<f64>,
    #[serde(default, deserialize_with = "crate::nullable")]
    request_id: Nullable<String>,
    is_sidechain: Option<bool>,
}

pub(super) fn is_valid_usage_record(data: &ClaudeUsageRecord) -> bool {
    if data
        .version
        .as_deref()
        .is_some_and(|version| !is_semver_prefix(version))
    {
        return false;
    }
    if data
        .session_id
        .as_deref()
        .is_some_and(|session_id| session_id.is_empty())
    {
        return false;
    }
    if data
        .request_id
        .as_deref()
        .is_some_and(|request_id| request_id.is_empty())
    {
        return false;
    }
    if data
        .message
        .id
        .as_deref()
        .is_some_and(|message_id| message_id.is_empty())
    {
        return false;
    }
    if data
        .message
        .model
        .as_deref()
        .is_some_and(|model| model.is_empty())
    {
        return false;
    }
    true
}

fn is_semver_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    if !consume_ascii_digits(bytes, &mut index) || bytes.get(index) != Some(&b'.') {
        return false;
    }
    index += 1;
    if !consume_ascii_digits(bytes, &mut index) || bytes.get(index) != Some(&b'.') {
        return false;
    }
    index += 1;
    bytes.get(index).is_some_and(u8::is_ascii_digit)
}

fn consume_ascii_digits(bytes: &[u8], index: &mut usize) -> bool {
    let start = *index;
    while bytes.get(*index).is_some_and(u8::is_ascii_digit) {
        *index += 1;
    }
    *index > start
}
