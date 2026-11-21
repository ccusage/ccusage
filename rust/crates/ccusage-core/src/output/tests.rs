use serde_json::json;

use super::{
    table::{empty_usage_table_message, format_last_activity_date, usage_table_columns},
    warnings::missing_pricing_warnings_for_models,
    *,
};
use crate::{ModelBreakdown, UsageSummary, cli::SharedArgs, resolve_tz};

#[test]
fn last_activity_column_shows_the_date_in_the_report_timezone() {
    let timezone = resolve_tz(Some(&"America/Los_Angeles".parse().unwrap()));

    assert_eq!(
        format_last_activity_date("2026-02-24T02:00:00.000Z", &timezone),
        "2026-02-23"
    );
    assert_eq!(
        format_last_activity_date("2026-02-24", &timezone),
        "2026-02-24"
    );
}

#[test]
fn narrow_non_tty_output_does_not_auto_compact() {
    let shared = SharedArgs::default();

    assert!(!should_use_compact_layout(&shared, false, 80, 100));
}

#[test]
fn narrow_tty_output_auto_compacts() {
    let shared = SharedArgs::default();

    assert!(should_use_compact_layout(&shared, true, 80, 100));
}

#[test]
fn compact_flag_forces_compact_for_non_tty_output() {
    let shared = SharedArgs {
        compact: true,
        ..SharedArgs::default()
    };

    assert!(should_use_compact_layout(&shared, false, 120, 100));
}

#[test]
fn empty_usage_table_message_is_provider_agnostic() {
    assert_eq!(empty_usage_table_message(), "No usage data found.");
}

#[test]
fn focused_table_can_omit_cache_creation_without_dropping_cache_reads() {
    let (headers, columns) = usage_table_columns("Date", false, false);

    assert_eq!(
        headers,
        vec![
            "Date",
            "Models",
            "Input",
            "Output",
            "Cache Read",
            "Total Tokens",
            "Cost (USD)",
        ]
    );
    assert_eq!(headers.len(), columns.len());
}

#[test]
fn focused_table_includes_cache_creation_by_default() {
    let options = UsageTableOptions::default();
    let (headers, columns) = usage_table_columns("Date", false, options.show_cache_creation);

    assert!(headers.contains(&"Cache Create"));
    assert_eq!(headers.len(), columns.len());
}

#[test]
fn totals_json_includes_extra_tokens_in_total() {
    let totals = totals_json(&[UsageSummary {
        date: Some("2026-01-02".to_string()),
        month: None,
        week: None,
        session_id: None,
        project_path: None,
        last_activity: None,
        first_activity: None,
        input_tokens: 100,
        output_tokens: 50,
        cache_creation_tokens: 10,
        cache_read_tokens: 5,
        extra_total_tokens: 7,
        total_cost: 0.25,
        credits: None,
        message_count: None,
        models_used: vec!["gpt-5".to_string()],
        model_breakdowns: Vec::new(),
        project: None,
        versions: None,
    }]);

    assert_eq!(totals["totalTokens"], 172);
    assert!(totals.get("unpricedModels").is_none());
}

fn summary_with_breakdowns(breakdowns: Vec<ModelBreakdown>) -> UsageSummary {
    UsageSummary {
        date: Some("2026-01-02".to_string()),
        month: None,
        week: None,
        session_id: None,
        project_path: None,
        last_activity: None,
        first_activity: None,
        input_tokens: 100,
        output_tokens: 50,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        extra_total_tokens: 0,
        total_cost: 0.25,
        credits: None,
        message_count: None,
        models_used: breakdowns
            .iter()
            .map(|breakdown| breakdown.model_name.clone())
            .collect(),
        model_breakdowns: breakdowns,
        project: None,
        versions: None,
    }
}

fn breakdown(model: &str, missing_pricing: bool) -> ModelBreakdown {
    ModelBreakdown {
        model_name: model.to_string(),
        input_tokens: 100,
        output_tokens: 50,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_1h_tokens: None,
        extra_total_tokens: 0,
        cost: if missing_pricing { 0.0 } else { 0.25 },
        missing_pricing,
        stored_cost: None,
        estimated_pricing: false,
    }
}

#[test]
fn totals_json_lists_unpriced_models_sorted_and_deduplicated() {
    let totals = totals_json(&[
        summary_with_breakdowns(vec![
            breakdown("zeta-new-model", true),
            breakdown("gpt-5", false),
        ]),
        summary_with_breakdowns(vec![
            breakdown("alpha-new-model", true),
            breakdown("zeta-new-model", true),
        ]),
    ]);

    assert_eq!(
        totals["unpricedModels"],
        json!(["alpha-new-model", "zeta-new-model"])
    );
}

#[test]
fn model_breakdown_json_marks_missing_pricing_only_when_true() {
    let priced = serde_json::to_value(breakdown("gpt-5", false)).unwrap();
    let unpriced = serde_json::to_value(breakdown("new-model", true)).unwrap();

    assert!(priced.get("missingPricing").is_none());
    assert_eq!(unpriced["missingPricing"], true);
}

#[test]
fn model_breakdown_json_includes_stored_cost_only_when_present() {
    let calculated = serde_json::to_value(breakdown("gpt-5", false)).unwrap();
    let stored = serde_json::to_value(ModelBreakdown {
        stored_cost: Some(0.5),
        ..breakdown("claude-sonnet-4-20250514", false)
    })
    .unwrap();

    assert!(calculated.get("storedCost").is_none());
    assert_eq!(stored["storedCost"], 0.5);
}

#[test]
fn strip_cost_json_removes_cost_fields_recursively() {
    let mut value = json!({
        "daily": [
            {
                "date": "2026-01-02",
                "totalTokens": 172,
                "totalCost": 0.25,
                "modelBreakdowns": [
                    {
                        "modelName": "gpt-5",
                        "costUSD": 0.1,
                        "storedCost": 0.1,
                        "inputTokens": 100,
                        "missingPricing": true
                    }
                ],
            }
        ],
        "blocks": [
            {
                "id": "2026-01-02T00:00:00.000Z",
                "costUSD": 1.5,
                "burnRate": {
                    "tokensPerMinute": 10,
                    "cost": 0.25
                },
                "projection": {
                    "totalTokens": 300,
                    "totalCost": 2.0
                }
            }
        ],
        "totals": {
            "totalTokens": 172,
            "totalCost": 0.25,
            "unpricedModels": ["gpt-5"]
        }
    });

    strip_cost_json(&mut value);

    assert_eq!(
        value,
        json!({
            "daily": [
                {
                    "date": "2026-01-02",
                    "totalTokens": 172,
                    "modelBreakdowns": [
                        {
                            "modelName": "gpt-5",
                            "inputTokens": 100,
                            "missingPricing": true
                        }
                    ],
                }
            ],
            "blocks": [
                {
                    "id": "2026-01-02T00:00:00.000Z",
                    "burnRate": {
                        "tokensPerMinute": 10
                    },
                    "projection": {
                        "totalTokens": 300
                    }
                }
            ],
            "totals": {
                "totalTokens": 172,
                "unpricedModels": ["gpt-5"]
            }
        })
    );
}

#[test]
fn missing_pricing_warnings_deduplicate_models() {
    assert_eq!(
        missing_pricing_warnings_for_models(
            ["gpt-5.2-codex", "claude-sonnet-4-20250514", "gpt-5.2-codex"],
            false
        ),
        vec![
            "WARN  Missing pricing for claude-sonnet-4-20250514; cost excludes this model. Update pricing or run again after LiteLLM has the model.",
            "WARN  Missing pricing for gpt-5.2-codex; cost excludes this model. Update pricing or run again after LiteLLM has the model.",
        ]
    );
}

#[test]
fn missing_pricing_warnings_mention_embedded_pricing_offline() {
    assert_eq!(
        missing_pricing_warnings_for_models(["gpt-5.2-codex"], true),
        vec![
            "WARN  Missing embedded pricing for gpt-5.2-codex; cost excludes this model. Run without --offline or update ccusage after pricing is added.",
        ]
    );
}

#[test]
fn snapshots_totals_json_with_extra_tokens_credits_and_zero_credit_omission() {
    let mut first = snapshot_summary("2026-01-02", Some("workspace/api"), Some(1.25));
    first.extra_total_tokens = 17;
    let mut second = snapshot_summary("2026-01-03", Some("workspace/web"), None);
    second.input_tokens = 0;
    second.output_tokens = 0;
    second.cache_creation_tokens = 0;
    second.cache_read_tokens = 0;
    second.extra_total_tokens = 3;
    second.total_cost = 0.0;

    insta::assert_json_snapshot!(totals_json(&[first, second]));
}

#[test]
fn snapshots_model_multiline_formatting_sorts_dedupes_and_shortens_names() {
    let models = vec![
        "claude-sonnet-4-20250514".to_string(),
        "gpt-5.2-codex".to_string(),
        "claude-sonnet-4-20250514".to_string(),
        "unknown".to_string(),
    ];

    insta::assert_snapshot!(format_models_multiline(&models));
}

#[test]
fn sanitizes_terminal_control_characters_as_visible_escapes() {
    assert_eq!(
        sanitize_terminal_text("future\nclient\t\u{1b}[31m"),
        r#"future\nclient\t\u{1b}[31m"#
    );
}

#[test]
fn model_breakdown_total_includes_extra_tokens() {
    let breakdown = ModelBreakdown {
        model_name: "model-a".to_string(),
        input_tokens: 100,
        output_tokens: 50,
        cache_creation_tokens: 10,
        cache_read_tokens: 5,
        cache_creation_1h_tokens: None,
        extra_total_tokens: 7,
        cost: 0.25,
        missing_pricing: false,
        stored_cost: None,
        estimated_pricing: false,
    };

    assert_eq!(breakdown.total_tokens(), 172);
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
                cache_creation_1h_tokens: None,
                extra_total_tokens: 0,
                cost: 0.3,
                missing_pricing: false,
                stored_cost: None,
                estimated_pricing: false,
            },
            ModelBreakdown {
                model_name: "claude-sonnet-4-20250514".to_string(),
                input_tokens: 334,
                output_tokens: 267,
                cache_creation_tokens: 39,
                cache_read_tokens: 0,
                cache_creation_1h_tokens: None,
                extra_total_tokens: 0,
                cost: 0.12,
                missing_pricing: false,
                stored_cost: None,
                estimated_pricing: false,
            },
        ],
        project: project.map(str::to_string),
        versions: Some(vec!["1.0.0".to_string(), "1.1.0".to_string()]),
    }
}
