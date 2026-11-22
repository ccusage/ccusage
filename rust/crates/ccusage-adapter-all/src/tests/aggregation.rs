use super::*;

fn test_agent_rows(agent: &'static str) -> AgentRows {
    AgentRows {
        rows: vec![SourceRow {
            agent,
            metadata: None,
            usage: RowUsage {
                period: "2026-01-02".to_string(),
                models_used: Vec::new(),
                input_tokens: 1,
                output_tokens: 0,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                total_tokens: 1,
                total_cost: 0.0,
                model_breakdowns: Vec::new(),
            },
        }],
        detected: true,
        diagnostics: Diagnostics::new(agent),
    }
}

#[test]
fn statusline_dispatch_rejects_a_non_codex_agent() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({"sessions/.keep": ""});
    Environment::with_vars([(
        "CODEX_HOME",
        Some(fixture.root().as_os_str().to_os_string()),
    )])
    .run(|| {
        let args = ccusage_cli::CodexStatuslineArgs {
            shared: SharedArgs {
                offline: true,
                ..SharedArgs::with_defaults()
            },
            session_id: None,
            codex_speed: ccusage_cli::CodexSpeed::Auto,
            mcp: false,
        };
        let error = runner(
            ccusage_cli::StandardAgent::Amp,
            ccusage_cli::AgentCommand::Statusline(args),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "The statusline runner is unavailable for amp."
        );
    })
}

#[test]
fn loads_agent_rows_concurrently() {
    let active_loaders = Arc::new(AtomicUsize::new(0));
    let specs = [
        ("claude", crate::progress::UsageLoadAgent("Claude")),
        ("codex", crate::progress::UsageLoadAgent("Codex")),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (agent, progress_agent))| {
        let active_loaders = Arc::clone(&active_loaders);
        AgentLoadSpec {
            index,
            agent,
            progress_agent,
            load: Box::new(move || {
                active_loaders.fetch_add(1, Ordering::AcqRel);
                let started = Instant::now();
                while active_loaders.load(Ordering::Acquire) < 2 {
                    if started.elapsed() > Duration::from_secs(1) {
                        return Err(crate::cli_error("agent loaders did not overlap"));
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                Ok(test_agent_rows(agent))
            }),
        }
    })
    .collect();
    let mut progress = crate::progress::UsageLoadProgress::new(false);

    let loaded = load_agent_rows_parallel(specs, &mut progress).unwrap();

    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].agent, "claude");
    assert_eq!(loaded[1].agent, "codex");
}

#[test]
fn aggregates_daily_agent_rows_by_period() {
    let rows = aggregate_periods(
        vec![
            SourceRow {
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
            },
            SourceRow {
                agent: "claude",
                metadata: None,
                usage: RowUsage {
                    period: "2026-01-02".to_string(),
                    models_used: vec!["claude-sonnet-4-20250514".to_string()],
                    input_tokens: 50,
                    output_tokens: 25,
                    cache_creation_tokens: 5,
                    cache_read_tokens: 3,
                    total_tokens: 83,
                    total_cost: 0.02,
                    model_breakdowns: Vec::new(),
                },
            },
        ],
        Bucket::Day,
    )
    .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].period, "2026-01-02");
    assert_eq!(rows[0].agent(), "all");
    assert_eq!(rows[0].input_tokens, 150);
    assert_eq!(rows[0].output_tokens, 45);
    assert_eq!(rows[0].cache_read_tokens, 13);
    assert_eq!(rows[0].total_tokens, 203);
    assert_eq!(
        rows[0].models_used,
        vec!["claude-sonnet-4-20250514".to_string(), "gpt-5".to_string()]
    );
    assert_eq!(
        rows[0].agents().map(<[_]>::to_vec),
        Some(vec!["claude", "codex"])
    );
    let breakdowns = rows[0].agent_breakdowns().unwrap();
    assert_eq!(breakdowns.len(), 2);
    assert_eq!(breakdowns[0].agent, "claude");
    assert_eq!(breakdowns[0].period, "2026-01-02");
    assert_eq!(breakdowns[1].agent, "codex");
}

#[test]
fn saturates_unified_daily_and_weekly_reports_for_extreme_counters() {
    let rows = vec![
        extreme_all_row("2099-01-02", u64::MAX),
        extreme_all_row("2099-01-03", 1),
    ];
    let daily_rows = aggregate_periods(rows.clone(), Bucket::Day).unwrap();
    let daily_report = report_json_with_agents(&daily_rows, AgentReportKind::Daily, false);
    let weekly_rows = aggregate_periods(rows, Bucket::Week(WeekDay::Monday)).unwrap();
    let weekly_report = report_json_with_agents(&weekly_rows, AgentReportKind::Weekly, false);

    for report in [daily_report, weekly_report] {
        for key in [
            "inputTokens",
            "outputTokens",
            "cacheCreationTokens",
            "cacheReadTokens",
            "totalTokens",
        ] {
            assert_eq!(report["totals"][key], u64::MAX, "{key}");
        }
    }
    assert_eq!(weekly_rows[0].model_breakdowns[0].input_tokens, u64::MAX);
    assert_eq!(weekly_rows[0].model_breakdowns[0].output_tokens, u64::MAX);
}

#[test]
fn buckets_unified_weekly_rows_by_requested_start_of_week() {
    let rows = vec![
        extreme_all_row("2026-01-04", 1),
        extreme_all_row("2026-01-05", 1),
    ];
    let periods = |start_of_week| {
        aggregate_periods(rows.clone(), Bucket::Week(start_of_week))
            .unwrap()
            .into_iter()
            .map(|row| row.period.clone())
            .collect::<Vec<_>>()
    };

    assert_eq!(periods(WeekDay::Monday), ["2025-12-29", "2026-01-05"]);
    assert_eq!(periods(WeekDay::Sunday), ["2026-01-04"]);
}

fn extreme_all_row(period: &str, tokens: u64) -> SourceRow {
    SourceRow {
        agent: "codex",
        metadata: None,
        usage: RowUsage {
            period: period.to_string(),
            models_used: vec!["gpt-5".to_string()],
            input_tokens: tokens,
            output_tokens: tokens,
            cache_creation_tokens: tokens,
            cache_read_tokens: tokens,
            total_tokens: tokens,
            total_cost: 0.0,
            model_breakdowns: vec![ModelBreakdown {
                model_name: "gpt-5".to_string(),
                input_tokens: tokens,
                output_tokens: tokens,
                cache_creation_tokens: tokens,
                cache_read_tokens: tokens,
                extra_total_tokens: tokens,
                ..ModelBreakdown::default()
            }],
        },
    }
}

#[test]
fn merges_same_agent_daily_rows_into_one_monthly_breakdown() {
    let rows = aggregate_periods(
        vec![
            SourceRow {
                agent: "claude",
                metadata: None,
                usage: RowUsage {
                    period: "2026-01-02".to_string(),
                    models_used: vec!["claude-sonnet-4-20250514".to_string()],
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_creation_tokens: 1,
                    cache_read_tokens: 2,
                    total_tokens: 18,
                    total_cost: 0.01,
                    model_breakdowns: vec![ModelBreakdown {
                        model_name: "claude-sonnet-4-20250514".to_string(),
                        input_tokens: 10,
                        output_tokens: 5,
                        cache_creation_tokens: 1,
                        cache_read_tokens: 2,
                        cost: 0.01,
                        ..ModelBreakdown::default()
                    }],
                },
            },
            SourceRow {
                agent: "claude",
                metadata: None,
                usage: RowUsage {
                    period: "2026-01-15".to_string(),
                    models_used: vec!["claude-opus-4-20250514".to_string()],
                    input_tokens: 20,
                    output_tokens: 10,
                    cache_creation_tokens: 2,
                    cache_read_tokens: 4,
                    total_tokens: 36,
                    total_cost: 0.05,
                    model_breakdowns: vec![ModelBreakdown {
                        model_name: "claude-opus-4-20250514".to_string(),
                        input_tokens: 20,
                        output_tokens: 10,
                        cache_creation_tokens: 2,
                        cache_read_tokens: 4,
                        cost: 0.05,
                        ..ModelBreakdown::default()
                    }],
                },
            },
            SourceRow {
                agent: "codex",
                metadata: None,
                usage: RowUsage {
                    period: "2026-01-20".to_string(),
                    models_used: vec!["gpt-5".to_string()],
                    input_tokens: 30,
                    output_tokens: 15,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 6,
                    total_tokens: 51,
                    total_cost: 0.02,
                    model_breakdowns: vec![ModelBreakdown {
                        model_name: "gpt-5".to_string(),
                        input_tokens: 30,
                        output_tokens: 15,
                        cache_creation_tokens: 0,
                        cache_read_tokens: 6,
                        cost: 0.02,
                        ..ModelBreakdown::default()
                    }],
                },
            },
        ],
        Bucket::Month,
    )
    .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].period, "2026-01");
    assert_eq!(rows[0].input_tokens, 60);
    assert_eq!(rows[0].output_tokens, 30);
    let breakdowns = rows[0].agent_breakdowns().unwrap();
    assert_eq!(
        breakdowns.len(),
        2,
        "expected one breakdown row per agent per month, got {breakdowns:#?}"
    );
    let claude = breakdowns
        .iter()
        .find(|row| row.agent == "claude")
        .expect("claude breakdown present");
    assert_eq!(claude.period, "2026-01");
    assert_eq!(claude.input_tokens, 30);
    assert_eq!(claude.output_tokens, 15);
    assert_eq!(claude.cache_creation_tokens, 3);
    assert_eq!(claude.cache_read_tokens, 6);
    assert!((claude.total_cost - 0.06).abs() < f64::EPSILON);
    assert_eq!(
        claude.models_used,
        vec![
            "claude-opus-4-20250514".to_string(),
            "claude-sonnet-4-20250514".to_string(),
        ]
    );
    assert_eq!(claude.model_breakdowns.len(), 2);
    assert_eq!(
        claude
            .model_breakdowns
            .iter()
            .map(|breakdown| breakdown.model_name.as_str())
            .collect::<Vec<_>>(),
        vec!["claude-opus-4-20250514", "claude-sonnet-4-20250514",]
    );
    assert_eq!(claude.model_breakdowns[0].cost, 0.05);
    assert_eq!(claude.model_breakdowns[1].cost, 0.01);
    let codex = breakdowns
        .iter()
        .find(|row| row.agent == "codex")
        .expect("codex breakdown present");
    assert_eq!(codex.input_tokens, 30);
}
