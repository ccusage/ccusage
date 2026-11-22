use std::fmt;

use crate::TimestampMs;

mod otel;
mod session_state;

#[cfg(test)]
pub(super) use otel::parse_otel_file;
pub(super) use otel::{OtelFileCalls, read_otel_calls, reconcile_otel_calls};
pub(super) use session_state::parse_session_state_file;

#[derive(Debug, Clone)]
pub(super) struct CopilotUsageEntry {
    pub(super) timestamp: TimestampMs,
    pub(super) session_id: String,
    pub(super) model: String,
    pub(super) logged_model: Option<String>,
    pub(super) input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) cache_creation_tokens: u64,
    pub(super) cache_read_tokens: u64,
    pub(super) reasoning_output_tokens: u64,
    pub(super) extra_total_tokens: u64,
    pub(super) fallback_output_tokens: u64,
    pub(super) request_count: u64,
    pub(super) nano_aiu: Option<u64>,
    pub(super) dedup_key: String,
    pub(super) call_ids: CallIds,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum CallKey {
    Span { trace: String, span: String },
    Log { trace: String, span: String },
    AgentTurn { trace: String, turn: u64 },
}

impl fmt::Display for CallKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Span { trace, span } => write!(formatter, "{trace}:{span}"),
            Self::Log { trace, span } => write!(formatter, "log:{trace}:{span}"),
            Self::AgentTurn { trace, turn } => write!(formatter, "agent-turn:{trace}:{turn}"),
        }
    }
}

// NOTE(2026-11-16, #2108): Copilot `apiCallId` corresponds to OTel `gen_ai.response.id` and `serviceRequestId` corresponds to `github.copilot.service_request_id`. Match each ID in its own namespace.
#[derive(Debug, Clone, Default)]
pub(super) struct CallIds {
    response_id: Option<String>,
    service_request_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum CallIdKind {
    Response,
    ServiceRequest,
}

impl CallIds {
    pub(super) fn iter(&self) -> impl Iterator<Item = (CallIdKind, &str)> {
        [
            (CallIdKind::Response, self.response_id.as_deref()),
            (
                CallIdKind::ServiceRequest,
                self.service_request_id.as_deref(),
            ),
        ]
        .into_iter()
        .filter_map(|(kind, id)| Some((kind, id?)))
    }
}

#[derive(Debug, Clone)]
pub(super) struct CopilotCreditSnapshot {
    pub(super) timestamp: TimestampMs,
    pub(super) session_id: String,
    pub(super) is_shutdown: bool,
    pub(super) total_nano_aiu: Option<u64>,
    pub(super) receipt_watermark: Option<u64>,
    pub(super) dedup_key: String,
}

#[derive(Debug, Clone)]
pub(super) struct CopilotUsageRecordEntry {
    pub(super) entry: CopilotUsageEntry,
    pub(super) source_session_id: Option<String>,
    pub(super) sequence: Option<u64>,
}

#[derive(Debug, Default)]
pub(super) struct CopilotSourceUsage {
    pub(super) otel_calls: Option<OtelFileCalls>,
    pub(super) entries: Vec<CopilotUsageEntry>,
    pub(super) usage_records: Vec<CopilotUsageRecordEntry>,
    pub(super) credit_snapshots: Vec<CopilotCreditSnapshot>,
}

fn normalize_copilot_model(model: &str) -> String {
    let model = model.trim();
    model
        .strip_suffix("-1m-internal")
        .or_else(|| model.strip_suffix("-1m"))
        .unwrap_or(model)
        .to_string()
}

fn copilot_model_names(logged_model: String) -> (String, Option<String>) {
    let model = normalize_copilot_model(&logged_model);
    let logged_model = (logged_model != model).then_some(logged_model);
    (model, logged_model)
}

#[cfg(test)]
mod session_state_tests;

#[cfg(test)]
mod pricing_identity_tests {
    use super::*;

    #[test]
    fn adapter_pricing_identity() {
        let model = normalize_copilot_model(" claude-opus-4.7-1m-internal ");
        assert_eq!(model, "claude-opus-4.7");
        assert_eq!(normalize_copilot_model("gpt-5.2"), "gpt-5.2");
    }
}
