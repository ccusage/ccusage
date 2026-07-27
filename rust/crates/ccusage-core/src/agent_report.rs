use serde_json::{Value, json};

use crate::{UsageSummary, cli::AgentReportKind};

pub fn agent_summary_json(
    row: &UsageSummary,
    kind: AgentReportKind,
    include_session_metadata: bool,
) -> Value {
    let mut value = json!({
        period_key(kind): summary_period(row),
        "inputTokens": row.input_tokens,
        "outputTokens": row.output_tokens,
        "cacheCreationTokens": row.cache_creation_tokens,
        "cacheReadTokens": row.cache_read_tokens,
        "totalTokens": row.total_tokens(),
        "totalCost": row.total_cost,
        "modelsUsed": row.models_used,
        "modelBreakdowns": row.model_breakdowns,
    });
    if let (Some(obj), Some(credits)) = (value.as_object_mut(), row.credits) {
        obj.insert("credits".to_string(), json!(credits));
    }
    if let (Some(obj), Some(message_count)) = (value.as_object_mut(), row.message_count) {
        obj.insert("messageCount".to_string(), json!(message_count));
    }
    if include_session_metadata && let Some(obj) = value.as_object_mut() {
        obj.insert(
            "lastActivity".to_string(),
            row.last_activity
                .as_ref()
                .map_or(Value::Null, |value| json!(value)),
        );
        obj.insert(
            "firstActivity".to_string(),
            row.first_activity
                .as_ref()
                .map_or(Value::Null, |value| json!(value)),
        );
        obj.insert(
            "projectPath".to_string(),
            row.project_path
                .as_ref()
                .map_or(Value::Null, |value| json!(value)),
        );
    }
    value
}

pub fn first_column(kind: AgentReportKind) -> &'static str {
    match kind {
        AgentReportKind::Daily => "Date",
        AgentReportKind::Weekly => "Week",
        AgentReportKind::Monthly => "Month",
        AgentReportKind::Session => "Session",
    }
}

pub fn summary_period(row: &UsageSummary) -> &str {
    row.date
        .as_deref()
        .or(row.week.as_deref())
        .or(row.month.as_deref())
        .or(row.session_id.as_deref())
        .unwrap_or_default()
}

fn period_key(kind: AgentReportKind) -> &'static str {
    match kind {
        AgentReportKind::Daily => "date",
        AgentReportKind::Weekly => "week",
        AgentReportKind::Monthly => "month",
        AgentReportKind::Session => "sessionId",
    }
}
