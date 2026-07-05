use std::{collections::BTreeMap, fs, path::Path, str::FromStr, sync::Arc};

use jiff::{Timestamp as JiffTimestamp, tz::TimeZone as JiffTimeZone};
use serde::Deserialize;

use super::super::jsonl;
use crate::{
    LoadedEntry, PricingMap, Result, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    calculate_cost_for_usage, cli::CostMode, format_date_tz, missing_pricing_model_for_usage,
    total_usage_tokens,
};

/// A Devin transcript JSON file following the ATIF trajectory schema with
/// Devin-specific `metadata`/`extra` extensions.
#[derive(Debug, Default, Deserialize)]
struct DevinTranscript {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    session_id: Option<String>,
    agent: Option<DevinAgent>,
    #[serde(default)]
    steps: Vec<DevinStep>,
}

#[derive(Debug, Default, Deserialize)]
struct DevinAgent {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    model_name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct DevinStep {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    timestamp: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    model_name: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    step_id: Option<String>,
    metrics: Option<DevinMetrics>,
    metadata: Option<DevinStepMetadata>,
    extra: Option<DevinStepExtra>,
}

#[derive(Debug, Default, Deserialize)]
struct DevinMetrics {
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    prompt_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    completion_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cached_tokens: u64,
    extra: Option<DevinMetricsExtra>,
}

#[derive(Debug, Default, Deserialize)]
struct DevinMetricsExtra {
    #[serde(
        rename = "cache_creation_input_tokens",
        default,
        deserialize_with = "jsonl::lenient_u64"
    )]
    cache_creation_input_tokens: u64,
}

#[derive(Debug, Default, Deserialize)]
struct DevinStepMetadata {
    #[serde(default)]
    is_user_input: bool,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    generation_model: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    created_at: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_f64")]
    committed_credit_cost: Option<f64>,
    metrics: Option<DevinLegacyMetrics>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    request_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct DevinLegacyMetrics {
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    input_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    output_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cache_creation_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cache_read_tokens: u64,
}

#[derive(Debug, Default, Deserialize)]
struct DevinStepExtra {
    #[serde(default, deserialize_with = "jsonl::lenient_f64")]
    committed_credit_cost: Option<f64>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    generation_model: Option<String>,
}

pub(crate) fn read_transcript_file(
    path: &Path,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
    session_info: Option<&SessionInfo>,
) -> Result<Vec<LoadedEntry>> {
    let content = fs::read(path)?;
    let transcript: DevinTranscript = serde_json::from_slice(&content)?;

    let session_id = transcript
        .session_id
        .clone()
        .or_else(|| session_info.map(|info| info.id.clone()))
        .unwrap_or_else(|| session_id_from_path(path));
    let fallback_model = transcript
        .agent
        .as_ref()
        .and_then(|agent| agent.model_name.clone());
    let project_path = session_info
        .and_then(|info| info.working_directory.as_deref())
        .unwrap_or("devin");
    let project_name = project_name_from_path(project_path);

    let mut entries = Vec::new();
    for step in transcript.steps {
        if skip_step(&step) {
            continue;
        }
        let Some(usage) = step_usage(&step) else {
            continue;
        };
        if total_usage_tokens(usage) == 0 {
            continue;
        }
        let Some(timestamp_text) = step_timestamp(&step, session_info) else {
            continue;
        };
        let Some(timestamp) = parse_iso_timestamp(&timestamp_text) else {
            continue;
        };
        let model = step_model(&step, &fallback_model, session_info);
        let cost_usd = step_credit_cost(&step);
        let cost = calculate_cost_for_usage(model.as_deref(), usage, cost_usd, mode, pricing);
        let missing_pricing_model =
            missing_pricing_model_for_usage(model.as_deref(), usage, cost_usd, mode, pricing);
        let data = UsageEntry {
            session_id: Some(session_id.clone()),
            timestamp: timestamp_text.clone(),
            version: None,
            message: UsageMessage {
                usage,
                model: model.clone(),
                id: step.request_id().or_else(|| step.step_id.clone()),
            },
            cost_usd,
            request_id: step.request_id(),
            is_api_error_message: None,
            is_sidechain: None,
        };
        entries.push(LoadedEntry {
            date: format_date_tz(timestamp, tz),
            timestamp,
            project: Arc::from(project_name.as_str()),
            session_id: Arc::from(session_id.as_str()),
            project_path: Arc::from(project_path),
            cost,
            extra_total_tokens: 0,
            credits: None,
            message_count: None,
            model,
            data,
            usage_limit_reset_time: None,
            missing_pricing_model,
        });
    }
    Ok(entries)
}

fn skip_step(step: &DevinStep) -> bool {
    step.metadata
        .as_ref()
        .is_some_and(|metadata| metadata.is_user_input)
}

fn step_usage(step: &DevinStep) -> Option<TokenUsageRaw> {
    if let Some(metrics) = step.metrics.as_ref() {
        let cache_creation = metrics
            .extra
            .as_ref()
            .map_or(0, |extra| extra.cache_creation_input_tokens);
        return Some(TokenUsageRaw {
            input_tokens: metrics.prompt_tokens,
            output_tokens: metrics.completion_tokens,
            cache_creation_input_tokens: cache_creation,
            cache_read_input_tokens: metrics.cached_tokens,
            speed: None,
            cache_creation: None,
        });
    }
    if let Some(metrics) = step
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.metrics.as_ref())
    {
        return Some(TokenUsageRaw {
            input_tokens: metrics.input_tokens,
            output_tokens: metrics.output_tokens,
            cache_creation_input_tokens: metrics.cache_creation_tokens,
            cache_read_input_tokens: metrics.cache_read_tokens,
            speed: None,
            cache_creation: None,
        });
    }
    None
}

fn step_timestamp(step: &DevinStep, session_info: Option<&SessionInfo>) -> Option<String> {
    step.metadata
        .as_ref()
        .and_then(|metadata| metadata.created_at.as_deref())
        .or(step.timestamp.as_deref())
        .map(|value| value.to_string())
        .or_else(|| {
            session_info.and_then(|info| info.last_activity_at.as_deref().map(String::from))
        })
        .or_else(|| session_info.and_then(|info| info.created_at.as_deref().map(String::from)))
}

fn step_model(
    step: &DevinStep,
    fallback_model: &Option<String>,
    session_info: Option<&SessionInfo>,
) -> Option<String> {
    step.metadata
        .as_ref()
        .and_then(|metadata| metadata.generation_model.as_deref())
        .or(step
            .extra
            .as_ref()
            .and_then(|extra| extra.generation_model.as_deref()))
        .or(step.model_name.as_deref())
        .or(fallback_model.as_deref())
        .or(session_info.and_then(|info| info.model.as_deref()))
        .map(|model| model.to_string())
}

fn step_credit_cost(step: &DevinStep) -> Option<f64> {
    step.metadata
        .as_ref()
        .and_then(|metadata| metadata.committed_credit_cost)
        .or(step
            .extra
            .as_ref()
            .and_then(|extra| extra.committed_credit_cost))
}

fn parse_iso_timestamp(value: &str) -> Option<TimestampMs> {
    JiffTimestamp::from_str(value)
        .ok()
        .map(|timestamp| TimestampMs::from_millis(timestamp.as_millisecond()))
}

fn session_id_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown")
        .to_string()
}

fn project_name_from_path(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_string()
}

impl DevinStep {
    fn request_id(&self) -> Option<String> {
        self.metadata
            .as_ref()
            .and_then(|metadata| metadata.request_id.clone())
    }
}

/// Enrichment data from Devin CLI `sessions.db` for a single transcript.
#[derive(Debug, Clone, Default)]
pub(crate) struct SessionInfo {
    pub(crate) id: String,
    pub(crate) working_directory: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) created_at: Option<String>,
    pub(crate) last_activity_at: Option<String>,
}

/// Load enrichment data from the Devin `sessions.db` SQLite database, if it
/// exists. Missing or unreadable databases return an empty map.
pub(crate) fn load_session_info(db_path: &Path) -> BTreeMap<String, SessionInfo> {
    let Ok(connection) =
        sqlite::Connection::open_with_flags(db_path, sqlite::OpenFlags::new().with_read_only())
    else {
        return BTreeMap::new();
    };
    let Ok(mut statement) = connection.prepare(
        "SELECT id, working_directory, model, created_at, last_activity_at FROM sessions WHERE hidden = 0 OR hidden IS NULL",
    ) else {
        return BTreeMap::new();
    };
    let mut info = BTreeMap::new();
    loop {
        match statement.next() {
            Ok(sqlite::State::Row) => {
                let Ok(id) = statement.read::<String, _>(0) else {
                    continue;
                };
                let working_directory = read_optional_string(&statement, 1);
                let model = read_optional_string(&statement, 2);
                let created_at = read_optional_timestamp(&statement, 3);
                let last_activity_at = read_optional_timestamp(&statement, 4);
                info.insert(
                    id.clone(),
                    SessionInfo {
                        id,
                        working_directory,
                        model,
                        created_at,
                        last_activity_at,
                    },
                );
            }
            Ok(sqlite::State::Done) => break,
            Err(_) => break,
        }
    }
    info
}

fn read_optional_string(statement: &sqlite::Statement, index: usize) -> Option<String> {
    statement
        .read::<String, _>(index)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn read_optional_timestamp(statement: &sqlite::Statement, index: usize) -> Option<String> {
    let value = statement.read::<sqlite::Value, _>(index).ok()?;
    match value {
        sqlite::Value::Integer(num) => Some(format_sqlite_timestamp(num)),
        sqlite::Value::String(text) => Some(text),
        _ => None,
    }
}

fn format_sqlite_timestamp(num: i64) -> String {
    let millis = if num < 10_000_000_000 {
        num * 1_000
    } else {
        num
    };
    crate::format_rfc3339_millis(crate::TimestampMs::from_millis(millis))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::fs_fixture;

    #[test]
    fn parses_atif_v1_7_metrics_and_credit_cost() {
        let fixture = fs_fixture!({
            "transcripts/session-a.json": r#"{
                "schema_version": "ATIF-v1.7",
                "session_id": "session-a",
                "agent": { "model_name": "claude-sonnet-4" },
                "steps": [
                    {
                        "step_id": "1",
                        "timestamp": "2026-01-02T00:00:00.000Z",
                        "source": "agent",
                        "metrics": {
                            "prompt_tokens": 100,
                            "completion_tokens": 50,
                            "cached_tokens": 25,
                            "extra": { "cache_creation_input_tokens": 10 }
                        },
                        "metadata": { "generation_model": "claude-sonnet-4-20250514", "committed_credit_cost": 0.05 }
                    }
                ]
            }"#,
        });
        let file = fixture.path("transcripts/session-a.json");

        let entries = read_transcript_file(&file, None, CostMode::Auto, None, None).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.input_tokens, 100);
        assert_eq!(entries[0].data.message.usage.output_tokens, 50);
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 25);
        assert_eq!(
            entries[0].data.message.usage.cache_creation_input_tokens,
            10
        );
        assert_eq!(entries[0].cost, 0.05);
        assert_eq!(
            entries[0].model.as_deref(),
            Some("claude-sonnet-4-20250514")
        );
    }

    #[test]
    fn falls_back_to_legacy_metadata_metrics() {
        let fixture = fs_fixture!({
            "transcripts/session-b.json": r#"{
                "session_id": "session-b",
                "steps": [
                    {
                        "step_id": "1",
                        "timestamp": "2026-01-02T00:00:00Z",
                        "metadata": {
                            "is_user_input": false,
                            "metrics": { "input_tokens": 10, "output_tokens": 20, "cache_creation_tokens": 5, "cache_read_tokens": 3 },
                            "generation_model": "swe"
                        }
                    }
                ]
            }"#,
        });
        let file = fixture.path("transcripts/session-b.json");

        let entries = read_transcript_file(&file, None, CostMode::Auto, None, None).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.input_tokens, 10);
        assert_eq!(entries[0].data.message.usage.output_tokens, 20);
        assert_eq!(entries[0].data.message.usage.cache_creation_input_tokens, 5);
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 3);
        assert_eq!(entries[0].model.as_deref(), Some("swe"));
    }

    #[test]
    fn skips_user_input_steps() {
        let fixture = fs_fixture!({
            "transcripts/session-c.json": r#"{
                "session_id": "session-c",
                "steps": [
                    {
                        "step_id": "1",
                        "timestamp": "2026-01-02T00:00:00Z",
                        "metadata": { "is_user_input": true, "metrics": { "input_tokens": 999, "output_tokens": 999 } }
                    },
                    {
                        "step_id": "2",
                        "timestamp": "2026-01-02T00:00:01Z",
                        "metadata": { "is_user_input": false, "metrics": { "input_tokens": 1, "output_tokens": 2 } }
                    }
                ]
            }"#,
        });
        let file = fixture.path("transcripts/session-c.json");

        let entries = read_transcript_file(&file, None, CostMode::Auto, None, None).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.input_tokens, 1);
    }
}
