use std::collections::BTreeMap;

use ccusage_core::UsageSummary;
use serde_json::{Value, json};

pub fn summary_json(row: &UsageSummary) -> Value {
    let mut value = json!({
        "inputTokens": row.input_tokens,
        "outputTokens": row.output_tokens,
        "cacheCreationTokens": row.cache_creation_tokens,
        "cacheReadTokens": row.cache_read_tokens,
        "totalTokens": row.total_tokens(),
        "totalCost": row.total_cost,
        "modelsUsed": row.models_used,
        "modelBreakdowns": row.model_breakdowns,
    });
    if let Some(obj) = value.as_object_mut() {
        if let Some(date) = &row.date {
            obj.insert("date".to_string(), json!(date));
        }
        if let Some(month) = &row.month {
            obj.insert("month".to_string(), json!(month));
        }
        if let Some(week) = &row.week {
            obj.insert("week".to_string(), json!(week));
        }
        if let Some(project) = &row.project {
            obj.insert("project".to_string(), json!(project));
        }
        if let Some(credits) = row.credits {
            obj.insert("credits".to_string(), json!(credits));
        }
    }
    value
}

pub fn session_summary_json(row: &UsageSummary) -> Value {
    let mut value = json!({
        "sessionId": row.session_id,
        "inputTokens": row.input_tokens,
        "outputTokens": row.output_tokens,
        "cacheCreationTokens": row.cache_creation_tokens,
        "cacheReadTokens": row.cache_read_tokens,
        "totalTokens": row.total_tokens(),
        "totalCost": row.total_cost,
        "lastActivity": row.last_activity,
        "firstActivity": row.first_activity,
        "modelsUsed": row.models_used,
        "modelBreakdowns": row.model_breakdowns,
        "projectPath": row.project_path,
    });
    if let (Some(obj), Some(credits)) = (value.as_object_mut(), row.credits) {
        obj.insert("credits".to_string(), json!(credits));
    }
    value
}

pub fn group_project_output(rows: &[UsageSummary]) -> Value {
    let mut projects: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for row in rows {
        projects
            .entry(row.project.clone().unwrap_or_else(|| "unknown".to_string()))
            .or_default()
            .push(summary_json(row));
    }
    json!(projects)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_core::ModelBreakdown;

    #[test]
    fn snapshots_summary_json_with_optional_fields_and_model_breakdowns() {
        let row = snapshot_summary("2026-01-02", Some("workspace/api"), Some(1.25));

        insta::assert_json_snapshot!(summary_json(&row));
    }

    #[test]
    fn snapshots_session_summary_json_with_present_and_missing_options() {
        let mut row = snapshot_summary("session-a", None, None);
        row.date = None;
        row.session_id = Some("session-a".to_string());
        row.project_path = Some("/Users/example/workspace/api".to_string());
        row.last_activity = Some("2026-01-02T12:34:56.000Z".to_string());
        row.first_activity = Some("2026-01-01T10:30:00.000Z".to_string());

        insta::assert_json_snapshot!(session_summary_json(&row));
    }

    #[test]
    fn snapshots_group_project_output_orders_named_projects_and_unknown_bucket() {
        let named = snapshot_summary("2026-01-02", Some("workspace/api"), Some(1.25));
        let unknown = snapshot_summary("2026-01-03", None, None);
        let other = snapshot_summary("2026-01-04", Some("workspace/web"), None);

        insta::assert_json_snapshot!(group_project_output(&[named, unknown, other]));
    }

    fn snapshot_summary(period: &str, project: Option<&str>, credits: Option<f64>) -> UsageSummary {
        UsageSummary {
            date: Some(period.to_string()),
            month: None,
            week: None,
            session_id: None,
            project_path: None,
            last_activity: None,
            first_activity: None,
            input_tokens: 1_234,
            output_tokens: 567,
            cache_creation_tokens: 89,
            cache_read_tokens: 10,
            extra_total_tokens: 0,
            total_cost: 0.42,
            credits,
            message_count: Some(7),
            models_used: vec![
                "gpt-5.2-codex".to_string(),
                "claude-sonnet-4-20250514".to_string(),
            ],
            model_breakdowns: vec![
                ModelBreakdown {
                    model_name: "gpt-5.2-codex".to_string(),
                    input_tokens: 900,
                    output_tokens: 300,
                    cache_creation_tokens: 50,
                    cache_read_tokens: 10,
                    extra_total_tokens: 0,
                    cost: 0.3,
                    missing_pricing: false,
                    stored_cost: None,
                },
                ModelBreakdown {
                    model_name: "claude-sonnet-4-20250514".to_string(),
                    input_tokens: 334,
                    output_tokens: 267,
                    cache_creation_tokens: 39,
                    cache_read_tokens: 0,
                    extra_total_tokens: 0,
                    cost: 0.12,
                    missing_pricing: false,
                    stored_cost: None,
                },
            ],
            project: project.map(str::to_string),
            versions: Some(vec!["1.0.0".to_string(), "1.1.0".to_string()]),
        }
    }
}
