use std::sync::LazyLock;

use ccusage_core::{TokenUsageRaw, fast::LinePrefilter};
use serde::Deserialize;

use super::usage_wire::{GuardedUsage, Rejection, deserialize_usage_line};

#[derive(Debug, Deserialize)]
struct UsageIterationsEnvelope {
    #[serde(default)]
    message: Option<UsageIterationsMessage>,
}

#[derive(Debug, Deserialize)]
struct ProgressIterationsEnvelope {
    data: ProgressIterationsData,
}

#[derive(Debug, Deserialize)]
struct ProgressIterationsData {
    message: UsageIterationsEnvelope,
}

#[derive(Debug, Deserialize)]
struct UsageIterationsMessage {
    #[serde(default)]
    usage: Option<UsageIterations>,
}

#[derive(Debug, Deserialize)]
struct UsageIterations {
    #[serde(default)]
    iterations: Vec<serde_json::Value>,
}

pub(crate) struct AdvisorUsage {
    pub(super) model: String,
    pub(super) usage: TokenUsageRaw,
}

#[derive(Default)]
pub(crate) struct AdvisorUsages {
    pub(super) usages: Vec<AdvisorUsage>,
    pub(super) malformed: usize,
}

pub(crate) fn advisor_usages_from_line(
    line: &[u8],
) -> std::result::Result<AdvisorUsages, Rejection> {
    static ADVISOR_MARKER: LazyLock<LinePrefilter> =
        LazyLock::new(|| LinePrefilter::all(&[br#""advisor_message""#]));
    if !ADVISOR_MARKER.matches(line) {
        return Ok(AdvisorUsages::default());
    }
    let envelope = deserialize_usage_line::<UsageIterationsEnvelope>(line)?;
    Ok(advisor_usages_from_envelope(envelope))
}

#[cold]
fn advisor_usages_from_progress_line(line: &[u8]) -> std::result::Result<AdvisorUsages, Rejection> {
    let envelope = deserialize_usage_line::<ProgressIterationsEnvelope>(line)?;
    Ok(advisor_usages_from_envelope(envelope.data.message))
}

fn advisor_usages_from_envelope(envelope: UsageIterationsEnvelope) -> AdvisorUsages {
    let iterations = envelope
        .message
        .and_then(|message| message.usage)
        .map(|usage| usage.iterations)
        .unwrap_or_default();
    let mut advisors = AdvisorUsages::default();
    for iteration in &iterations {
        let text = |key: &str| iteration.get(key).and_then(serde_json::Value::as_str);
        // NOTE(2026-09-16, Claude Code 2.1.268, #1719): Claude Code 2.1.266 onward can write a null model on Fable 5.1 main-model iterations. Require a model only for advisor usage.
        let Some(model) = text("model").filter(|model| !model.is_empty()) else {
            continue;
        };
        // NOTE(2026-07-10, #1423): Claude Code reports advisor usage separately from the top-level main-model totals. Count only advisor iterations in this additional pass.
        if text("type") != Some("advisor_message") {
            continue;
        }
        let Ok(usage) = GuardedUsage::deserialize(iteration) else {
            advisors.malformed += 1;
            continue;
        };
        if usage.has_null {
            continue;
        }
        if ccusage_core::checked_total_usage_tokens(usage.usage.usage, 0).is_none() {
            advisors.malformed += 1;
            continue;
        }
        advisors.usages.push(AdvisorUsage {
            model: model.to_owned(),
            usage: usage.usage.usage,
        });
    }
    advisors
}

#[derive(Debug, Clone, Copy)]
pub(super) enum AdvisorLookup {
    RuledOut,
    AskLine,
    AskProgress,
}

impl AdvisorLookup {
    pub(super) fn usages(self, line: &[u8]) -> std::result::Result<AdvisorUsages, Rejection> {
        match self {
            Self::RuledOut => Ok(AdvisorUsages::default()),
            Self::AskLine => advisor_usages_from_line(line),
            Self::AskProgress => advisor_usages_from_progress_line(line),
        }
    }
}
