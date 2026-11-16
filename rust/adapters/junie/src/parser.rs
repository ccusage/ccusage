use std::collections::HashMap;

use ccusage_adapter_common::jsonl;
use serde::Deserialize;

use crate::{TimestampMs, TokenUsageRaw, fast::LinePrefilter};

const USAGE_EVENT_KIND: &str = "LlmResponseMetadataEvent";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct SessionMeta {
    pub(super) created_at: Option<TimestampMs>,
    pub(super) project_dir: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct JunieUsage {
    pub(super) timestamp: TimestampMs,
    pub(super) model: String,
    pub(super) usage: TokenUsageRaw,
    pub(super) cost: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexLine {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    session_id: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_i64")]
    created_at: Option<i64>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    project_dir: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventLine {
    event: Option<EventBody>,
    #[serde(default, deserialize_with = "jsonl::lenient_i64")]
    timestamp_ms: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventBody {
    agent_event: Option<AgentEvent>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentEvent {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    kind: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_vec")]
    model_usage: Vec<ModelUsage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelUsage {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    model: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    input_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    output_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cache_input_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cache_create_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_f64")]
    cost: Option<f64>,
}

/// Parses `sessions/index.jsonl`; a later line for the same session wins
/// because Junie appends updated metadata.
pub(super) fn parse_session_index(content: &[u8]) -> HashMap<String, SessionMeta> {
    jsonl::records::<IndexLine>(content, None)
        .filter_map(|line| {
            let session_id = line.session_id?;
            Some((
                session_id,
                SessionMeta {
                    created_at: line
                        .created_at
                        .filter(|millis| *millis > 0)
                        .map(TimestampMs::from_millis),
                    project_dir: line.project_dir,
                },
            ))
        })
        .collect()
}

/// Parses one session's `events.jsonl` into one usage record per
/// `modelUsage` entry.
///
/// Builds before `timestampMs` existed wrote usage events without a time, so
/// those fall back to the session's `createdAt`; with neither, the record is
/// skipped rather than dated by a guess.
pub(super) fn parse_events(
    content: &[u8],
    session_created_at: Option<TimestampMs>,
) -> Vec<JunieUsage> {
    // JSON may spell any letter of the kind as `\uXXXX`, so lines with a
    // `\u` escape always reach the parser where the decoded kind is compared.
    let prefilter = LinePrefilter::any(&[USAGE_EVENT_KIND.as_bytes(), br"\u"]);
    jsonl::records::<EventLine>(content, Some(&prefilter))
        .flat_map(|line| {
            let timestamp = line
                .timestamp_ms
                .filter(|millis| *millis > 0)
                .map(TimestampMs::from_millis)
                .or(session_created_at);
            let usages = line
                .event
                .and_then(|event| event.agent_event)
                .filter(|agent_event| agent_event.kind.as_deref() == Some(USAGE_EVENT_KIND))
                .map(|agent_event| agent_event.model_usage)
                .unwrap_or_default();
            timestamp
                .map(|timestamp| {
                    usages
                        .into_iter()
                        .filter_map(move |usage| usage_record(timestamp, usage))
                })
                .into_iter()
                .flatten()
        })
        .collect()
}

fn usage_record(timestamp: TimestampMs, usage: ModelUsage) -> Option<JunieUsage> {
    let model = usage.model?;
    let tokens = TokenUsageRaw {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_creation_input_tokens: usage.cache_create_tokens,
        cache_read_input_tokens: usage.cache_input_tokens,
        ..TokenUsageRaw::default()
    };
    if crate::total_usage_tokens(tokens) == 0 {
        return None;
    }
    Some(JunieUsage {
        timestamp,
        model,
        usage: tokens,
        cost: usage.cost.filter(|cost| cost.is_finite() && *cost >= 0.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CURRENT: &str = r#"{"kind":"SessionA2uxEvent","event":{"state":"IN_PROGRESS","agentEvent":{"kind":"LlmResponseMetadataEvent","agent":{"kind":"MainAgent","id":"main","name":"main","type":"LINEAR"},"modelUsage":[{"model":"gpt-4.1-2025-04-14","cost":0.00225,"inputTokens":1121,"cacheInputTokens":0,"cacheCreateTokens":0,"outputTokens":1},{"model":"claude-sonnet-4-20250514","cost":0.0004,"inputTokens":800,"cacheInputTokens":100,"cacheCreateTokens":50,"outputTokens":42}]}},"taskId":"task-1","timestampMs":1785470253773}"#;
    const LEGACY: &str = r#"{"kind":"SessionA2uxEvent","event":{"state":"IN_PROGRESS","agentEvent":{"kind":"LlmResponseMetadataEvent","modelUsage":[{"model":"gpt-4.1-2025-04-14","cost":0.00225,"inputTokens":1121,"outputTokens":1}]}}}"#;

    #[test]
    fn expands_each_model_usage_entry_into_a_record() {
        let records = parse_events(CURRENT.as_bytes(), None)
            .into_iter()
            .map(|record| {
                (
                    record.timestamp.as_millis(),
                    record.model,
                    record.usage.input_tokens,
                    record.usage.output_tokens,
                    record.usage.cache_read_input_tokens,
                    record.usage.cache_creation_input_tokens,
                    record.cost,
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            records,
            vec![
                (
                    1_785_470_253_773,
                    "gpt-4.1-2025-04-14".to_string(),
                    1121,
                    1,
                    0,
                    0,
                    Some(0.00225)
                ),
                (
                    1_785_470_253_773,
                    "claude-sonnet-4-20250514".to_string(),
                    800,
                    42,
                    100,
                    50,
                    Some(0.0004)
                ),
            ]
        );
    }

    #[test]
    fn event_timestamp_wins_over_session_creation_time() {
        let records = parse_events(
            CURRENT.as_bytes(),
            Some(TimestampMs::from_millis(1_700_000_000_000)),
        );

        assert!(
            records
                .iter()
                .all(|record| record.timestamp.as_millis() == 1_785_470_253_773)
        );
    }

    #[test]
    fn legacy_events_use_session_creation_time() {
        let created_at = TimestampMs::from_millis(1_779_231_600_000);
        let records = parse_events(LEGACY.as_bytes(), Some(created_at));

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].timestamp, created_at);
    }

    #[test]
    fn legacy_events_without_session_creation_time_are_skipped() {
        assert!(parse_events(LEGACY.as_bytes(), None).is_empty());
    }

    #[test]
    fn reads_usage_events_whose_kind_is_json_escaped() {
        let content = r#"{"event":{"agentEvent":{"kind":"LlmResponseMetadata\u0045vent","modelUsage":[{"model":"gpt-4.1","inputTokens":7}]}},"timestampMs":1785470253773}"#;

        assert!(
            !content.contains(USAGE_EVENT_KIND),
            "fixture must not contain the raw kind"
        );
        let records = parse_events(content.as_bytes(), None);

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].usage.input_tokens, 7);
    }

    #[test]
    fn ignores_other_events_and_unusable_usage_entries() {
        let content = [
            "not json",
            r#"{"kind":"SessionA2uxEvent","event":{"agentEvent":{"kind":"LlmResponseMetadataEvent""#,
            r#"{"kind":"SessionA2uxEvent","event":{"agentEvent":{"kind":"AgentStateUpdatedEvent","note":"LlmResponseMetadataEvent","modelUsage":[{"model":"gpt-4.1","inputTokens":5}]}},"timestampMs":1785470253773}"#,
            r#"{"event":{"agentEvent":{"kind":"LlmResponseMetadataEvent","modelUsage":"nope"}},"timestampMs":1785470253773}"#,
            r#"{"event":{"agentEvent":{"kind":"LlmResponseMetadataEvent","modelUsage":[{"inputTokens":5},{"model":"gpt-4.1","inputTokens":0,"outputTokens":0,"cost":0.1},"oops",{"model":"gpt-4.1","inputTokens":7,"cost":-1}]}},"timestampMs":1785470253773}"#,
        ]
        .join("\n");

        let records = parse_events(content.as_bytes(), None);

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].model, "gpt-4.1");
        assert_eq!(records[0].usage.input_tokens, 7);
        assert_eq!(records[0].cost, None);
    }

    #[test]
    fn index_maps_session_ids_to_creation_time_and_project() {
        let content = [
            r#"{"sessionId":"session-a","createdAt":1785470000000,"updatedAt":1785470278054,"projectDir":"/work/app","taskName":"Implement feature"}"#,
            r#"{"sessionId":"session-b","projectDir":""}"#,
            r#"{"createdAt":1785470000000}"#,
            "not json",
        ]
        .join("\n");

        let index = parse_session_index(content.as_bytes());

        assert_eq!(index.len(), 2);
        assert_eq!(
            index["session-a"],
            SessionMeta {
                created_at: Some(TimestampMs::from_millis(1_785_470_000_000)),
                project_dir: Some("/work/app".to_string()),
            }
        );
        assert_eq!(index["session-b"], SessionMeta::default());
    }
}
