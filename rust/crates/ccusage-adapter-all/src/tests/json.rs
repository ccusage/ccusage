use super::*;

#[test]
fn renders_all_report_json_with_period_and_agent_metadata() {
    let rows = vec![ReportRow::Period(PeriodRow {
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
        agent_breakdowns: Vec::new(),
    })];

    let report = report_json_with_agents(&rows, AgentReportKind::Daily, false);

    assert_eq!(report["daily"][0]["period"], "2026-01-02");
    assert_eq!(report["daily"][0]["agent"], "all");
    assert_eq!(report["daily"][0]["metadata"]["agents"], json!(["codex"]));
    assert_eq!(report["totals"]["totalTokens"], 130);
}

#[test]
fn all_report_totals_list_unpriced_models_only_when_present() {
    let mut row = ReportRow::Period(PeriodRow {
        usage: RowUsage {
            period: "2026-01-02".to_string(),
            models_used: vec!["new-model".to_string(), "gpt-5".to_string()],
            input_tokens: 100,
            output_tokens: 20,
            cache_creation_tokens: 0,
            cache_read_tokens: 10,
            total_tokens: 130,
            total_cost: 0.01,
            model_breakdowns: vec![
                ModelBreakdown {
                    model_name: "new-model".to_string(),
                    input_tokens: 50,
                    output_tokens: 10,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                    cache_creation_1h_tokens: None,
                    extra_total_tokens: 0,
                    cost: 0.0,
                    missing_pricing: true,
                    stored_cost: None,
                    estimated_pricing: false,
                },
                ModelBreakdown {
                    model_name: "gpt-5".to_string(),
                    input_tokens: 50,
                    output_tokens: 10,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 10,
                    cache_creation_1h_tokens: None,
                    extra_total_tokens: 0,
                    cost: 0.01,
                    missing_pricing: false,
                    stored_cost: None,
                    estimated_pricing: false,
                },
            ],
        },
        agents: Vec::new(),
        agent_breakdowns: Vec::new(),
    });

    let report = report_json_with_agents(std::slice::from_ref(&row), AgentReportKind::Daily, false);
    assert_eq!(report["totals"]["unpricedModels"], json!(["new-model"]));
    assert_eq!(
        report["daily"][0]["modelBreakdowns"][0]["missingPricing"],
        true
    );
    assert!(
        report["daily"][0]["modelBreakdowns"][1]
            .get("missingPricing")
            .is_none()
    );

    let nested = report_json_with_agents(
        &[ReportRow::Period(PeriodRow {
            usage: (*row).clone(),
            agents: Vec::new(),
            agent_breakdowns: vec![SourceRow {
                agent: "codex",
                usage: (*row).clone(),
                metadata: None,
            }],
        })],
        AgentReportKind::Daily,
        true,
    );
    assert_eq!(
        nested["daily"][0]["agents"][0]["modelBreakdowns"][0]["missingPricing"],
        true
    );

    let priced_session = ReportRow::Source(SourceRow {
        usage: RowUsage {
            period: "session-a".to_string(),
            model_breakdowns: vec![row.model_breakdowns[1].clone()],
            ..(*row).clone()
        },
        agent: row.clone().agent(),
        metadata: None,
    });
    let sections = sections_report_json(
        &[
            (AgentReportKind::Daily, vec![row.clone()]),
            (AgentReportKind::Session, vec![priced_session]),
        ],
        AgentReportKind::Session,
        false,
    );
    let sections = serde_json::to_value(&sections).unwrap();
    assert!(
        sections["totals"].get("unpricedModels").is_none(),
        "sections totals follow the invoked section only"
    );

    let unpriced_session = ReportRow::Source(SourceRow {
        usage: RowUsage {
            period: "session-b".to_string(),
            ..(*row).clone()
        },
        agent: row.clone().agent(),
        metadata: None,
    });
    let sections = sections_report_json(
        &[
            (AgentReportKind::Daily, Vec::new()),
            (AgentReportKind::Session, vec![unpriced_session]),
        ],
        AgentReportKind::Session,
        false,
    );
    let sections = serde_json::to_value(&sections).unwrap();
    assert_eq!(
        sections["totals"]["unpricedModels"],
        json!(["new-model"]),
        "the invoked section's unpriced models reach totals"
    );

    row.model_breakdowns.remove(0);
    let report = report_json_with_agents(std::slice::from_ref(&row), AgentReportKind::Daily, false);
    assert!(report["totals"].get("unpricedModels").is_none());
}

#[test]
fn renders_by_agent_json_breakdowns_when_requested() {
    let rows = vec![ReportRow::Period(PeriodRow {
        usage: RowUsage {
            period: "2026-01-02".to_string(),
            models_used: vec!["claude-sonnet-4-20250514".to_string(), "gpt-5".to_string()],
            input_tokens: 150,
            output_tokens: 45,
            cache_creation_tokens: 5,
            cache_read_tokens: 13,
            total_tokens: 203,
            total_cost: 0.03,
            model_breakdowns: Vec::new(),
        },
        agents: vec!["claude", "codex"],
        agent_breakdowns: vec![
            SourceRow {
                usage: RowUsage {
                    period: "2026-01-02".to_string(),
                    models_used: vec!["claude-sonnet-4-20250514".to_string()],
                    input_tokens: 50,
                    output_tokens: 25,
                    cache_creation_tokens: 5,
                    cache_read_tokens: 3,
                    total_tokens: 83,
                    total_cost: 0.02,
                    model_breakdowns: vec![ModelBreakdown {
                        model_name: "claude-sonnet-4-20250514".to_string(),
                        input_tokens: 50,
                        output_tokens: 25,
                        cache_creation_tokens: 5,
                        cache_read_tokens: 3,
                        cost: 0.02,
                        ..ModelBreakdown::default()
                    }],
                },
                agent: "claude",
                metadata: None,
            },
            SourceRow {
                usage: RowUsage {
                    period: "2026-01-02".to_string(),
                    models_used: vec!["gpt-5".to_string()],
                    input_tokens: 100,
                    output_tokens: 20,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 10,
                    total_tokens: 120,
                    total_cost: 0.01,
                    model_breakdowns: vec![ModelBreakdown {
                        model_name: "gpt-5".to_string(),
                        input_tokens: 100,
                        output_tokens: 20,
                        cache_creation_tokens: 0,
                        cache_read_tokens: 10,
                        cost: 0.01,
                        ..ModelBreakdown::default()
                    }],
                },
                agent: "codex",
                metadata: None,
            },
        ],
    })];

    let report = report_json_with_agents(&rows, AgentReportKind::Daily, true);

    assert_eq!(report["daily"][0]["agents"][0]["agent"], "claude");
    assert_eq!(report["daily"][0]["agents"][0]["inputTokens"], 50);
    assert_eq!(report["daily"][0]["agents"][1]["agent"], "codex");
    assert_eq!(report["daily"][0]["agents"][1]["totalCost"], 0.01);
    assert_eq!(
        report["daily"][0]["agents"][1]["modelBreakdowns"][0]["modelName"],
        "gpt-5"
    );
    let agents = report["daily"][0]["agents"].as_array().unwrap();
    assert_eq!(
        agents
            .iter()
            .map(|agent| agent["inputTokens"].as_u64().unwrap())
            .sum::<u64>(),
        report["daily"][0]["inputTokens"]
    );
    assert_eq!(
        agents
            .iter()
            .map(|agent| agent["outputTokens"].as_u64().unwrap())
            .sum::<u64>(),
        report["daily"][0]["outputTokens"]
    );
    let agent_cost = agents
        .iter()
        .map(|agent| agent["totalCost"].as_f64().unwrap())
        .sum::<f64>();
    let row_cost = report["daily"][0]["totalCost"].as_f64().unwrap();
    assert!((agent_cost - row_cost).abs() < f64::EPSILON);
}

#[test]
fn omits_by_agent_json_breakdowns_by_default() {
    let rows = aggregate_periods(
        vec![SourceRow {
            agent: "codex",
            metadata: None,
            usage: RowUsage {
                period: "2026-01-02".to_string(),
                models_used: vec!["gpt-5".to_string()],
                input_tokens: 100,
                output_tokens: 20,
                cache_creation_tokens: 0,
                cache_read_tokens: 10,
                total_tokens: 120,
                total_cost: 0.01,
                model_breakdowns: Vec::new(),
            },
        }],
        Bucket::Day,
    )
    .unwrap();

    let report = report_json_with_agents(&rows, AgentReportKind::Daily, false);

    assert!(report["daily"][0].get("agents").is_none());
}

#[test]
fn renders_multi_section_json_with_command_totals() {
    let daily_rows = vec![ReportRow::Period(PeriodRow {
        usage: RowUsage {
            period: "2026-01-02".to_string(),
            models_used: vec!["gpt-5".to_string()],
            input_tokens: 100,
            output_tokens: 20,
            cache_creation_tokens: 0,
            cache_read_tokens: 10,
            total_tokens: 120,
            total_cost: 0.01,
            model_breakdowns: Vec::new(),
        },
        agents: vec!["codex"],
        agent_breakdowns: Vec::new(),
    })];
    let monthly_rows = vec![ReportRow::Period(PeriodRow {
        usage: RowUsage {
            period: "2026-01".into(),
            ..(*daily_rows[0]).clone()
        },
        agents: vec!["codex"],
        agent_breakdowns: Vec::new(),
    })];
    let session_rows = vec![ReportRow::Source(SourceRow {
        usage: RowUsage {
            period: "session-a".to_string(),
            models_used: vec!["gpt-5".to_string()],
            input_tokens: 100,
            output_tokens: 20,
            cache_creation_tokens: 0,
            cache_read_tokens: 10,
            total_tokens: 120,
            total_cost: 0.01,
            model_breakdowns: Vec::new(),
        },
        agent: "codex",
        metadata: Some(ccusage_adapter_common::rows::SessionMetadata {
            last_activity: Some(Some("2026-01-02T00:00:00.000Z".into())),
            ..Default::default()
        }),
    })];
    let sections = vec![
        (AgentReportKind::Daily, daily_rows.clone()),
        (AgentReportKind::Monthly, monthly_rows.clone()),
        (AgentReportKind::Session, session_rows.clone()),
    ];

    let report = sections_report_json(&sections, AgentReportKind::Daily, false);
    let value = serde_json::to_value(&report).unwrap();

    assert_eq!(
        value["daily"],
        report_json_with_agents(&daily_rows, AgentReportKind::Daily, false)["daily"]
    );
    assert_eq!(
        value["monthly"],
        report_json_with_agents(&monthly_rows, AgentReportKind::Monthly, false)["monthly"]
    );
    assert_eq!(
        value["session"],
        report_json_with_agents(&session_rows, AgentReportKind::Session, false)["session"]
    );
    assert_eq!(
        value["totals"],
        report_json_with_agents(&daily_rows, AgentReportKind::Daily, false)["totals"]
    );
    insta::assert_snapshot!(serde_json::to_string_pretty(&report).unwrap());
}

#[test]
fn renders_multi_section_json_keys_in_invoked_section_order_with_totals_last() {
    let sections = vec![
        (AgentReportKind::Weekly, Vec::new()),
        (AgentReportKind::Daily, Vec::new()),
        (AgentReportKind::Monthly, Vec::new()),
        (AgentReportKind::Session, Vec::new()),
    ];

    let report = sections_report_json(&sections, AgentReportKind::Weekly, false);
    let serialized = serde_json::to_string(&report).unwrap();

    assert_eq!(
        serialized,
        r#"{"weekly":[],"daily":[],"monthly":[],"session":[],"totals":{"cacheCreationTokens":0,"cacheReadTokens":0,"inputTokens":0,"outputTokens":0,"totalCost":0,"totalTokens":0}}"#
    );
}

#[test]
fn unified_sessions_sort_by_cost_in_json_and_table() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    for (id, cost) in [("a-cheap", 1.0), ("m-medium", 3.0), ("z-expensive", 20.0)] {
        fixture.write_file(
            format!("projects/project-a/{id}.jsonl"),
            json!({
                "timestamp": "2099-01-02T00:00:00.000Z",
                "sessionId": id,
                "requestId": id,
                "costUSD": cost,
                "message": {
                    "id": id,
                    "model": "claude-sonnet-4-20250514",
                    "usage": {"input_tokens": 100, "output_tokens": 50}
                }
            })
            .to_string(),
        );
    }
    isolated_agent_env(
        &fixture,
        "CLAUDE_CONFIG_DIR",
        fixture.root().as_os_str().into(),
    )
    .run(|| {
        let shared = SharedArgs {
            mode: CostMode::Display,
            ..fixture_shared("20990101", "20990103")
        };

        for (order, order_explicit, expected) in [
            (
                crate::cli::SortOrder::Asc,
                false,
                ["z-expensive", "m-medium", "a-cheap"],
            ),
            (
                crate::cli::SortOrder::Asc,
                true,
                ["a-cheap", "m-medium", "z-expensive"],
            ),
            (
                crate::cli::SortOrder::Desc,
                true,
                ["z-expensive", "m-medium", "a-cheap"],
            ),
        ] {
            let shared = SharedArgs {
                order: order_explicit.then_some(order),
                ..shared.clone()
            };
            let result = load_rows(
                AgentReportKind::Session,
                &shared,
                &NO_SOURCES,
                WeekDay::Monday,
            )
            .unwrap();
            let report = report_json_with_agents(&result.rows, AgentReportKind::Session, false);
            let ids: Vec<_> = report["session"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["period"].as_str().unwrap())
                .collect();
            assert_eq!(ids, expected);
            assert_eq!(report["totals"]["totalCost"], 24.0);

            let table = table_snapshot(
                &result.rows,
                AgentReportKind::Session,
                &result.detected_agents,
            );
            let table_ids: Vec<_> = table["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["cells"][0].as_str().unwrap())
                .collect();
            assert_eq!(table_ids, [expected[0], expected[1], expected[2], "Total"]);
            assert_daily_family_and_session_sections_match_standalone(&shared);
        }
    })
}

#[test]
fn multi_section_claude_fixture_matches_standalone_sections_for_daily_and_session_invocations() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "projects/project-a/session-a.jsonl": [
            r#"{"timestamp":"2099-01-02T00:00:00.000Z","sessionId":"session-a","requestId":"req-direct","costUSD":0.01,"message":{"usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":0,"cache_read_input_tokens":0},"model":"claude-sonnet-4-20250514","id":"msg-direct"}}"#,
            r#"{"data":{"message":{"timestamp":"2099-01-02T00:01:00.000Z","requestId":"req-progress","isSidechain":true,"costUSD":0.02,"message":{"usage":{"input_tokens":1000,"output_tokens":500,"cache_creation_input_tokens":0,"cache_read_input_tokens":0},"model":"claude-sonnet-4-20250514","id":"msg-progress"}}}}"#,
        ]
        .join("\n"),
    });
    isolated_agent_env(
        &fixture,
        "CLAUDE_CONFIG_DIR",
        fixture.root().as_os_str().into(),
    )
    .run(|| {
        let shared = fixture_shared("20990102", "20990102");

        assert_daily_family_and_session_sections_match_standalone(&shared);
    })
}

#[test]
fn all_report_totals_list_estimated_models_only_when_present() {
    let breakdown = |model: &str, estimated_pricing| ModelBreakdown {
        model_name: model.to_string(),
        input_tokens: 50,
        output_tokens: 10,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_1h_tokens: None,
        extra_total_tokens: 0,
        cost: 0.01,
        missing_pricing: false,
        stored_cost: None,
        estimated_pricing,
    };
    let row = |breakdowns| {
        ReportRow::Period(PeriodRow {
            usage: RowUsage {
                period: "2026-01-02".to_string(),
                models_used: vec!["kyma/glm-4.7-flash".to_string(), "gpt-5".to_string()],
                input_tokens: 100,
                output_tokens: 20,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                total_tokens: 120,
                total_cost: 0.02,
                model_breakdowns: breakdowns,
            },
            agents: vec!["opencode", "codex"],
            agent_breakdowns: Vec::new(),
        })
    };

    let estimated = row(vec![
        breakdown("kyma/glm-4.7-flash", true),
        breakdown("gpt-5", false),
    ]);
    let report = report_json_with_agents(
        std::slice::from_ref(&estimated),
        AgentReportKind::Daily,
        false,
    );
    assert_eq!(
        report["totals"]["estimatedModels"],
        json!(["kyma/glm-4.7-flash"])
    );
    assert_eq!(
        report["daily"][0]["modelBreakdowns"][0]["estimatedPricing"],
        true
    );

    let priced = row(vec![
        breakdown("kyma/glm-4.7-flash", false),
        breakdown("gpt-5", false),
    ]);
    let report =
        report_json_with_agents(std::slice::from_ref(&priced), AgentReportKind::Daily, false);
    assert!(report["totals"].get("estimatedModels").is_none());
}
