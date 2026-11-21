use std::{borrow::Cow, sync::Arc};

use ccusage_adapter_common::json::{Fields, JsonText, Member};
use serde::Deserialize;

use crate::{
    GuardedUsage, IteratedTokenUsage, Nullable, Rejection, TimestampMs, TokenUsageRaw,
    advisor::AdvisorLookup,
    cli::IsoDate,
    dedupe::{DedupEntry, DedupKey, Request},
    non_null,
    usage_wire::{Text, deserialize_borrowed_usage_line},
};

mod links;

pub(super) use links::{parse_block_links, parse_line_links};

#[derive(Debug)]
pub(super) struct ClaudeEntry {
    pub(super) key: DedupKey,
    pub(super) is_sidechain: bool,
    pub(super) date: IsoDate,
    pub(super) project: Arc<str>,
    pub(super) usage: TokenUsageRaw,
    pub(super) cost: f64,
    pub(super) model: Option<Arc<str>>,
    pub(super) missing_pricing: bool,
    pub(super) recorded_cost: RecordedCost,
}

const _: () = assert!(size_of::<ClaudeEntry>() == 192);

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
        self.is_sidechain
    }

    fn usage(&self) -> &TokenUsageRaw {
        &self.usage
    }

    fn records_cost(&self) -> bool {
        !matches!(self.recorded_cost, RecordedCost::Absent)
    }
}

#[derive(Debug)]
pub(crate) struct ClaudeUsageRecord<'a> {
    pub(super) timestamp: Cow<'a, str>,
    pub(crate) message: ClaudeUsageMessage,
    pub(super) version: Option<Cow<'a, str>>,
    pub(super) session_id: Option<Cow<'a, str>>,
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawUsageRecord<'a> {
    #[serde(borrow)]
    timestamp: Text<'a>,
    message: RawUsageMessage,
    #[serde(borrow, default, deserialize_with = "crate::nullable")]
    version: Nullable<Text<'a>>,
    #[serde(borrow, default, deserialize_with = "crate::nullable")]
    session_id: Nullable<Text<'a>>,
    #[serde(rename = "costUSD")]
    #[serde(default, deserialize_with = "crate::nullable")]
    cost_usd: Nullable<f64>,
    #[serde(default, deserialize_with = "crate::nullable")]
    request_id: Nullable<String>,
    is_sidechain: Option<bool>,
    #[serde(default, deserialize_with = "crate::nullable")]
    is_api_error_message: Nullable<bool>,
}

impl<'a> RawUsageRecord<'a> {
    fn decode(self) -> std::result::Result<ClaudeUsageRecord<'a>, Rejection> {
        Ok(ClaudeUsageRecord {
            timestamp: self.timestamp.0,
            message: self.message.decode()?,
            version: non_null(self.version)?.map(|version| version.0),
            session_id: non_null(self.session_id)?.map(|session_id| session_id.0),
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

const USAGE_MESSAGE: Fields = &[
    ("usage", Member::Value),
    ("model", Member::Value),
    ("id", Member::Value),
];

// NOTE(2026-11-21): Usage lines spend most of their bytes in message content that no report reads. These lists name every member `RawUsageRecord` and `AgentProgressEntry` read, so the picker skips the rest unparsed.
const USAGE_LINE: Fields = &[
    ("timestamp", Member::Value),
    ("message", Member::Object(USAGE_MESSAGE)),
    ("version", Member::Value),
    ("sessionId", Member::Value),
    ("costUSD", Member::Value),
    ("requestId", Member::Value),
    ("isSidechain", Member::Value),
    ("isApiErrorMessage", Member::Value),
    (
        "data",
        Member::Object(&[(
            "message",
            Member::Object(&[
                ("isApiErrorMessage", Member::Value),
                ("timestamp", Member::Value),
                ("message", Member::Object(USAGE_MESSAGE)),
                ("costUSD", Member::Value),
                ("requestId", Member::Value),
                ("isSidechain", Member::Value),
            ]),
        )]),
    ),
];

pub(crate) fn parse_usage_line<'a>(
    line: &'a JsonText<'a>,
) -> std::result::Result<(ClaudeUsageRecord<'a>, AdvisorLookup), Rejection> {
    match line.pick(USAGE_LINE) {
        Some(picked) => decode_usage_line(|| picked.parse(), || picked.parse()),
        None => parse_full_usage_line(line),
    }
}

fn parse_full_usage_line<'a>(
    line: &'a JsonText<'a>,
) -> std::result::Result<(ClaudeUsageRecord<'a>, AdvisorLookup), Rejection> {
    decode_usage_line(
        || deserialize_borrowed_usage_line(line),
        || deserialize_borrowed_usage_line(line),
    )
}

fn decode_usage_line<'a, E>(
    direct: impl FnOnce() -> std::result::Result<RawUsageRecord<'a>, E>,
    progress: impl FnOnce() -> std::result::Result<AgentProgressEntry<'a>, E>,
) -> std::result::Result<(ClaudeUsageRecord<'a>, AdvisorLookup), Rejection> {
    match direct() {
        Ok(raw) => {
            let entry = raw.decode()?;
            let lookup = if entry.message.usage.has_advisor_iteration {
                AdvisorLookup::AskLine
            } else {
                AdvisorLookup::RuledOut
            };
            Ok((entry, lookup))
        }
        Err(_) => progress()
            .map_err(|_| Rejection::Malformed)
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
#[derive(Deserialize)]
struct AgentProgressEntry<'a> {
    #[serde(rename = "sessionId")]
    #[serde(borrow, default, deserialize_with = "crate::nullable")]
    session_id: Nullable<Text<'a>>,
    #[serde(borrow)]
    data: AgentProgressData<'a>,
}

impl<'a> AgentProgressEntry<'a> {
    fn decode(self) -> std::result::Result<ClaudeUsageRecord<'a>, Rejection> {
        let message = self.data.message;
        Ok(ClaudeUsageRecord {
            timestamp: message.timestamp.0,
            message: message.message.decode()?,
            version: None,
            is_api_error_message: non_null(message.is_api_error_message)?,
            session_id: non_null(self.session_id)?.map(|session_id| session_id.0),
            cost_usd: non_null(message.cost_usd)?,
            request_id: non_null(message.request_id)?,
            is_sidechain: message.is_sidechain,
        })
    }
}

#[derive(Deserialize)]
struct AgentProgressData<'a> {
    #[serde(borrow)]
    message: AgentProgressMessage<'a>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentProgressMessage<'a> {
    #[serde(default, deserialize_with = "crate::nullable")]
    is_api_error_message: Nullable<bool>,
    #[serde(borrow)]
    timestamp: Text<'a>,
    message: RawUsageMessage,
    #[serde(rename = "costUSD")]
    #[serde(default, deserialize_with = "crate::nullable")]
    cost_usd: Nullable<f64>,
    #[serde(default, deserialize_with = "crate::nullable")]
    request_id: Nullable<String>,
    is_sidechain: Option<bool>,
}

pub(super) fn is_valid_usage_record(data: &ClaudeUsageRecord<'_>) -> bool {
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

#[cfg(test)]
mod tests;
