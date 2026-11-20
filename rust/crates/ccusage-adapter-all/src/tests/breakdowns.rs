use super::*;

#[test]
fn uses_non_cached_codex_input_tokens_in_all_rows() {
    let mut group = CodexGroup {
        input_tokens: 100,
        cached_input_tokens: 70,
        cache_creation_tokens: 20,
        output_tokens: 5,
        total_tokens: 105,
        ..CodexGroup::default()
    };
    let mut usage = CodexModelUsage::from_buckets(
        ccusage_adapter_codex::CodexUsageBucket {
            input_tokens: 100,
            cached_input_tokens: 70,
            cache_creation_tokens: 20,
            output_tokens: 5,
            ..Default::default()
        },
        Default::default(),
    );
    usage.total_tokens = 105;
    group.models.insert("gpt-5".to_string(), usage);
    let row = adapter::codex::unified_row_from_group(
        ccusage_adapter_common::source::LoadKind::Session,
        "2026-01-02",
        &group,
        Some(&PricingMap::default()),
        adapter::codex::CodexSpeedPolicy::Forced(adapter::codex::CodexServiceTier::Standard),
        CostMode::Calculate,
    );

    assert_eq!(row.input_tokens, 10);
    assert_eq!(row.cache_creation_tokens, 20);
    assert_eq!(row.cache_read_tokens, 70);
    assert_eq!(row.total_tokens, 105);
}

#[test]
fn includes_codex_model_breakdowns_in_all_rows() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
            "gpt-5": {
                "input_cost_per_token": 0.000001,
                "output_cost_per_token": 0.000010,
                "cache_read_input_token_cost": 0.0000001
            },
            "gpt-5-mini": {
                "input_cost_per_token": 0.0000001,
                "output_cost_per_token": 0.000001,
                "cache_read_input_token_cost": 0.00000001
            }
        }"#,
    );
    let mut group = CodexGroup {
        input_tokens: 300,
        cached_input_tokens: 100,
        output_tokens: 50,
        total_tokens: 350,
        ..CodexGroup::default()
    };
    let mut usage = CodexModelUsage::from_buckets(
        ccusage_adapter_codex::CodexUsageBucket {
            input_tokens: 100,
            cached_input_tokens: 20,
            output_tokens: 10,
            ..Default::default()
        },
        Default::default(),
    );
    usage.total_tokens = 110;
    group.models.insert("gpt-5-mini".to_string(), usage);
    let mut usage = CodexModelUsage::from_buckets(
        ccusage_adapter_codex::CodexUsageBucket {
            input_tokens: 200,
            cached_input_tokens: 80,
            output_tokens: 40,
            ..Default::default()
        },
        Default::default(),
    );
    usage.total_tokens = 240;
    group.models.insert("gpt-5".to_string(), usage);

    let row = adapter::codex::unified_row_from_group(
        ccusage_adapter_common::source::LoadKind::Session,
        "2026-01-02",
        &group,
        Some(&pricing),
        adapter::codex::CodexSpeedPolicy::Forced(adapter::codex::CodexServiceTier::Standard),
        CostMode::Calculate,
    );

    assert_eq!(row.model_breakdowns.len(), 2);
    assert_eq!(row.model_breakdowns[0].model_name, "gpt-5");
    assert_eq!(row.model_breakdowns[0].input_tokens, 120);
    assert_eq!(row.model_breakdowns[0].cache_read_tokens, 80);
    assert_eq!(row.model_breakdowns[0].output_tokens, 40);
    assert_eq!(row.model_breakdowns[1].model_name, "gpt-5-mini");
}

#[test]
fn display_mode_omits_codex_missing_pricing_from_unified_json() {
    let mut group = CodexGroup::default();
    let mut usage = CodexModelUsage::from_buckets(
        ccusage_adapter_codex::CodexUsageBucket {
            input_tokens: 100,
            ..Default::default()
        },
        Default::default(),
    );
    usage.total_tokens = 100;
    group
        .models
        .insert("gpt-unknown-preview".to_string(), usage);
    let row = adapter::codex::unified_row_from_group(
        ccusage_adapter_common::source::LoadKind::Session,
        "2026-01-02",
        &group,
        Some(&PricingMap::default()),
        adapter::codex::CodexSpeedPolicy::Forced(adapter::codex::CodexServiceTier::Standard),
        CostMode::Display,
    );

    let report = report_json_with_agents(&[ReportRow::Source(row)], AgentReportKind::Daily, false);

    assert!(report["totals"].get("unpricedModels").is_none());
    assert!(
        report["daily"][0]["modelBreakdowns"][0]
            .get("missingPricing")
            .is_none()
    );
}

#[test]
fn aggregates_model_breakdowns_across_agents() {
    let rows = aggregate_periods(
        vec![
            SourceRow {
                agent: "codex",
                metadata: None,
                usage: RowUsage {
                    period: "2026-01-02".to_string(),
                    models_used: vec!["gpt-5".to_string()],
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 2,
                    total_tokens: 17,
                    total_cost: 0.03,
                    model_breakdowns: vec![
                        ModelBreakdown {
                            model_name: "gpt-5".to_string(),
                            input_tokens: 10,
                            output_tokens: 5,
                            cache_creation_tokens: 0,
                            cache_read_tokens: 2,
                            cost: 0.03,
                            ..ModelBreakdown::default()
                        },
                        ModelBreakdown {
                            model_name: "gpt-5-mini".to_string(),
                            input_tokens: 1,
                            ..ModelBreakdown::default()
                        },
                    ],
                },
            },
            SourceRow {
                agent: "claude",
                metadata: None,
                usage: RowUsage {
                    period: "2026-01-02".to_string(),
                    models_used: vec!["gpt-5".to_string(), "claude-sonnet-4-20250514".to_string()],
                    input_tokens: 30,
                    output_tokens: 20,
                    cache_creation_tokens: 3,
                    cache_read_tokens: 4,
                    total_tokens: 57,
                    total_cost: 0.07,
                    model_breakdowns: vec![
                        ModelBreakdown {
                            model_name: "gpt-5".to_string(),
                            input_tokens: 8,
                            output_tokens: 3,
                            cache_creation_tokens: 1,
                            cache_read_tokens: 2,
                            cache_creation_1h_tokens: Some(1),
                            cost: 0.01,
                            missing_pricing: true,
                            ..ModelBreakdown::default()
                        },
                        ModelBreakdown {
                            model_name: "claude-sonnet-4-20250514".to_string(),
                            input_tokens: 22,
                            output_tokens: 17,
                            cache_creation_tokens: 2,
                            cache_read_tokens: 2,
                            cost: 0.06,
                            ..ModelBreakdown::default()
                        },
                        ModelBreakdown {
                            model_name: "gpt-5-mini".to_string(),
                            input_tokens: 2,
                            ..ModelBreakdown::default()
                        },
                    ],
                },
            },
        ],
        Bucket::Day,
    )
    .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].model_breakdowns.len(), 3);
    assert_eq!(
        rows[0].model_breakdowns[0].model_name,
        "claude-sonnet-4-20250514"
    );
    assert_eq!(rows[0].model_breakdowns[0].cost, 0.06);
    assert_eq!(rows[0].model_breakdowns[1].model_name, "gpt-5");
    assert_eq!(rows[0].model_breakdowns[1].input_tokens, 18);
    assert_eq!(rows[0].model_breakdowns[1].output_tokens, 8);
    assert_eq!(rows[0].model_breakdowns[1].cache_creation_tokens, 1);
    assert_eq!(rows[0].model_breakdowns[1].cache_read_tokens, 4);
    assert_eq!(rows[0].model_breakdowns[1].cost, 0.04);
    assert!(rows[0].model_breakdowns[1].missing_pricing);
    assert_eq!(
        rows[0].model_breakdowns[1].cache_creation_1h_tokens,
        Some(1)
    );
    assert_eq!(rows[0].model_breakdowns[2].model_name, "gpt-5-mini");
    assert_eq!(rows[0].model_breakdowns[2].input_tokens, 3);
    assert_eq!(rows[0].model_breakdowns[2].cache_creation_1h_tokens, None);
}

#[test]
fn displays_total_tokens_including_extra_tokens() {
    let row = ReportRow::Source(SourceRow {
        usage: RowUsage {
            period: "2026-01-02".to_string(),
            models_used: vec!["gpt-5".to_string()],
            input_tokens: 100,
            output_tokens: 20,
            cache_creation_tokens: 0,
            cache_read_tokens: 10,
            total_tokens: 135,
            total_cost: 0.01,
            model_breakdowns: Vec::new(),
        },
        agent: "codex",
        metadata: None,
    });

    let cells = all_table_row(&row, false, false, false);

    assert_eq!(cells[7], "135");
}

#[test]
fn all_table_rows_match_main_agent_breakdown_display() {
    let row = ReportRow::Period(PeriodRow {
        usage: RowUsage {
            period: "2026-01-02".to_string(),
            models_used: vec!["gpt-5".to_string()],
            input_tokens: 100,
            output_tokens: 20,
            cache_creation_tokens: 0,
            cache_read_tokens: 10,
            total_tokens: 130,
            total_cost: 0.01,
            model_breakdowns: Vec::new(),
        },
        agents: vec!["codex"],
        agent_breakdowns: vec![SourceRow {
            usage: RowUsage {
                period: "2026-01-02".to_string(),
                models_used: vec!["gpt-5".to_string()],
                input_tokens: 100,
                output_tokens: 20,
                cache_creation_tokens: 0,
                cache_read_tokens: 10,
                total_tokens: 130,
                total_cost: 0.01,
                model_breakdowns: Vec::new(),
            },
            agent: "codex",
            metadata: None,
        }],
    });

    assert_eq!(
        all_table_row(&row, true, false, false),
        vec!["2026-01-02", "All", "", "100", "20", "$0.01"]
    );
    assert_eq!(
        source_table_row(
            row.agent_breakdowns().unwrap().first().unwrap(),
            true,
            true,
            false,
        ),
        vec!["", "- Codex", "- gpt-5", "100", "20", "$0.01"]
    );
}

#[test]
fn all_report_title_lists_detected_agents() {
    assert_eq!(
        all_report_title(
            AgentReportKind::Daily,
            &["pi", "codex", "amp", "claude", "opencode"],
        ),
        "Coding (Agent) CLI Usage Report - Daily\nDetected: Amp, Claude, Codex, OpenCode, pi-agent"
    );
    assert_eq!(
        all_report_title(AgentReportKind::Session, &[]),
        "Coding (Agent) CLI Usage Report - Session\nDetected: None"
    );
}

#[test]
fn compact_table_columns_omit_cache_and_total_token_metrics() {
    let (headers, columns) = all_table_columns(AgentReportKind::Daily, true, false);

    assert_eq!(
        headers,
        vec!["Date", "Agent", "Models", "Input", "Output", "Cost (USD)"]
    );
    assert_eq!(
        columns,
        vec![
            ColumnKind::Primary,
            ColumnKind::Detail,
            ColumnKind::Models,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
            ColumnKind::Numeric,
        ]
    );
}

#[test]
fn full_table_columns_include_cache_and_total_token_metrics() {
    let (headers, columns) = all_table_columns(AgentReportKind::Daily, false, false);

    assert_eq!(
        headers,
        vec![
            "Date",
            "Agent",
            "Models",
            "Input",
            "Output",
            "Cache Create",
            "Cache Read",
            "Total Tokens",
            "Cost (USD)",
        ]
    );
    assert_eq!(headers.len(), columns.len());
}
