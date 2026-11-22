use super::*;

#[test]
fn multi_section_pi_load_uses_source_options_for_daily_and_session_rows() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "custom/pi/sessions/project-a/agent_in-range.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-source-config","usage":{"input":1000,"output":500}}}"#,
        "custom/pi/sessions/project-b/agent_out-of-range.jsonl": r#"{"type":"message","timestamp":"2099-01-03T00:00:00.000Z","message":{"role":"assistant","model":"gpt-source-config","usage":{"input":10000,"output":5000}}}"#,
    });
    isolated_agent_env(
        &fixture,
        "PI_AGENT_DIR",
        fixture.path("empty-default").into_os_string(),
    )
    .run(|| {
        let shared = SharedArgs {
            mode: CostMode::Calculate,
            ..fixture_shared("20990101", "20990104")
        };
        let sources = std::collections::BTreeMap::from([(
            "pi".to_string(),
            SourceArgs {
                options: ccusage_core::cli::AgentOptions {
                    pi_path: Some(
                        fixture
                            .path("custom/pi/sessions")
                            .to_string_lossy()
                            .into_owned(),
                    ),
                    ..Default::default()
                },
                shared: SharedArgs {
                    since: Some("20990102".parse().unwrap()),
                    until: Some("20990102".parse().unwrap()),
                    pricing_overrides: std::collections::BTreeMap::from([(
                        "gpt-source-config".to_string(),
                        crate::cli::PricingOverride {
                            input_cost_per_token: Some(1e-6),
                            output_cost_per_token: Some(2e-6),
                            ..Default::default()
                        },
                    )]),
                    ..shared.clone()
                },
            },
        )]);

        let sections = load_sections(
            &[AgentReportKind::Daily, AgentReportKind::Session],
            &shared,
            &sources,
            WeekDay::Monday,
        )
        .unwrap();
        let daily_rows = &sections.sections[0].1;
        assert_eq!(daily_rows.len(), 1);
        assert_eq!(daily_rows[0].period, "2099-01-02");
        assert_eq!(daily_rows[0].agents().map(<[_]>::to_vec), Some(vec!["pi"]));
        assert_eq!(daily_rows[0].input_tokens, 1_000);
        assert_eq!(daily_rows[0].output_tokens, 500);
        assert!((daily_rows[0].total_cost - 0.002).abs() < f64::EPSILON);

        let session_rows = &sections.sections[1].1;
        assert_eq!(session_rows.len(), 1);
        assert_eq!(session_rows[0].period, "in-range");
        assert_eq!(session_rows[0].agent(), "pi");
        assert_eq!(
            session_rows[0]
                .metadata()
                .unwrap()
                .project_path
                .as_deref()
                .unwrap(),
            "project-a"
        );
        assert_eq!(session_rows[0].input_tokens, 1_000);
        assert_eq!(session_rows[0].output_tokens, 500);
        assert!((session_rows[0].total_cost - 0.002).abs() < f64::EPSILON);
    })
}

#[test]
fn bounded_codex_reports_deduplicate_per_bucket_after_unified_bucketing() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    let usage = |timestamp: &str| {
        json!({"timestamp":timestamp,"type":"event_msg","payload":{"type":"token_count","info":{"model":"gpt-5","last_token_usage":{"input_tokens":100,"output_tokens":10,"total_tokens":110}}}}).to_string()
    };
    fixture.write_file(
        "codex/sessions/original.jsonl",
        usage("2026-01-02T00:00:00.000Z"),
    );
    fixture.write_file("codex/sessions/copy.jsonl", usage("2026-01-02T00:00:00Z"));
    fixture.write_file(
        "codex/sessions/later.jsonl",
        usage("2026-01-03T00:00:00.000Z"),
    );
    isolated_agent_env(
        &fixture,
        "CODEX_HOME",
        fixture.path("codex").into_os_string(),
    )
    .run(|| {
        let shared = fixture_shared("20260102", "20260102");
        for kind in [
            AgentReportKind::Daily,
            AgentReportKind::Weekly,
            AgentReportKind::Monthly,
            AgentReportKind::Session,
        ] {
            let unified = load_rows(kind, &shared, &NO_SOURCES, WeekDay::Monday)
                .unwrap()
                .rows;
            assert_eq!(unified.len(), 1, "{kind:?}");
            for row in unified {
                assert_eq!(
                    (
                        row.input_tokens,
                        row.output_tokens,
                        row.cache_creation_tokens,
                        row.cache_read_tokens,
                        row.total_tokens
                    ),
                    (100, 10, 0, 0, 110),
                    "{kind:?}",
                );
                assert!((row.total_cost - 0.000225).abs() < 1e-12, "{kind:?}");
            }
        }
    })
}

#[test]
fn multi_section_codex_fixture_matches_standalone_sections_for_daily_and_session_invocations() {
    ccusage_test_support::install_pricing_snapshots();
    let _aliases = set_model_aliases_for_tests([("private-alpha", "gpt-5.2")]);
    let duplicate_session_usage = codex_usage_line("2099-02-01T08:01:00.000Z", "gpt-5.2", 1_000);
    let alias_usage = codex_usage_line("2099-02-02T08:01:00.000Z", "private-alpha", 2_000);
    let canonical_usage = codex_usage_line("2099-02-02T08:01:00.000Z", "gpt-5.2", 2_000);
    let fixture = fs_fixture!({
        "codex/sessions/session-a.jsonl": &duplicate_session_usage,
        "codex/sessions/session-b.jsonl": &duplicate_session_usage,
        "codex/sessions/alias-a.jsonl": &alias_usage,
        "codex/sessions/alias-b.jsonl": &canonical_usage,
    });
    isolated_agent_env(
        &fixture,
        "CODEX_HOME",
        fixture.path("codex").into_os_string(),
    )
    .run(|| {
        let shared = fixture_shared("20990201", "20990202");

        assert_daily_family_and_session_sections_match_standalone(&shared);
    })
}

#[test]
fn zcode_fixture_reports_daily_monthly_session_json_and_table_snapshots() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    fixture.create_dir_all("zcode/cli/db");
    let db_path = fixture.path("zcode/cli/db/db.sqlite");
    #[derive(Clone, Copy)]
    struct FixtureUsage<'a> {
        id: &'a str,
        session_id: &'a str,
        timestamp: &'a str,
        model: &'a str,
        input_tokens: i64,
        output_tokens: i64,
        cache_creation_tokens: i64,
        cache_read_tokens: i64,
        computed_total_tokens: i64,
    }

    let db = sqlite::open(db_path).unwrap();
    db.execute(
        "CREATE TABLE model_usage (
            id TEXT PRIMARY KEY, session_id TEXT, started_at INTEGER, model_id TEXT,
            provider_id TEXT, status TEXT, input_tokens INTEGER, output_tokens INTEGER,
            cache_creation_input_tokens INTEGER, cache_read_input_tokens INTEGER,
            computed_total_tokens INTEGER
        )",
    )
    .unwrap();
    db.execute("CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT, version TEXT)")
        .unwrap();
    db.execute(
        "INSERT INTO session VALUES
            ('session-a', '/workspace/project-a', '0.16.3'),
            ('session-b', '/workspace/project-b', '0.16.3')",
    )
    .unwrap();
    insert_usage(
        &db,
        FixtureUsage {
            id: "usage-52",
            session_id: "session-a",
            timestamp: "2099-01-02T00:00:00.000Z",
            model: "GLM-5.2",
            input_tokens: 100,
            output_tokens: 10,
            cache_creation_tokens: 15,
            cache_read_tokens: 25,
            computed_total_tokens: 110,
        },
    );
    insert_usage(
        &db,
        FixtureUsage {
            id: "usage-53",
            session_id: "session-a",
            timestamp: "2099-01-15T12:00:00.000Z",
            model: "GLM-5.3",
            input_tokens: 200,
            output_tokens: 20,
            cache_creation_tokens: 30,
            cache_read_tokens: 40,
            computed_total_tokens: 220,
        },
    );
    insert_usage(
        &db,
        FixtureUsage {
            id: "usage-53-b",
            session_id: "session-b",
            timestamp: "2099-02-01T00:00:00.000Z",
            model: "GLM-5.3",
            input_tokens: 50,
            output_tokens: 5,
            cache_creation_tokens: 0,
            cache_read_tokens: 10,
            computed_total_tokens: 55,
        },
    );

    fn insert_usage(db: &sqlite::Connection, usage: FixtureUsage<'_>) {
        let mut statement = db
            .prepare(
                "INSERT INTO model_usage
             (id, session_id, started_at, model_id, provider_id, status, input_tokens,
              output_tokens, cache_creation_input_tokens, cache_read_input_tokens,
              computed_total_tokens)
             VALUES (?1, ?2, ?3, ?4, 'builtin:zai-coding-plan', 'completed', ?5, ?6, ?7, ?8, ?9)",
            )
            .unwrap();
        statement.bind((1, usage.id)).unwrap();
        statement.bind((2, usage.session_id)).unwrap();
        statement
            .bind((3, ccusage_test_support::instant(usage.timestamp).millis()))
            .unwrap();
        statement.bind((4, usage.model)).unwrap();
        statement.bind((5, usage.input_tokens)).unwrap();
        statement.bind((6, usage.output_tokens)).unwrap();
        statement.bind((7, usage.cache_creation_tokens)).unwrap();
        statement.bind((8, usage.cache_read_tokens)).unwrap();
        statement.bind((9, usage.computed_total_tokens)).unwrap();
        statement.next().unwrap();
    }

    isolated_agent_env(
        &fixture,
        "ZCODE_HOME",
        fixture.path("zcode").into_os_string(),
    )
    .run(|| {
        let mut shared = fixture_shared("20990101", "20990201");
        shared.mode = CostMode::Calculate;

        let daily = load_rows(
            AgentReportKind::Daily,
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        )
        .unwrap();
        let monthly = load_rows(
            AgentReportKind::Monthly,
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        )
        .unwrap();
        let session = load_rows(
            AgentReportKind::Session,
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        )
        .unwrap();

        assert_eq!(daily.detected_agents, vec!["zcode"]);
        assert_eq!(monthly.detected_agents, vec!["zcode"]);
        assert_eq!(session.detected_agents, vec!["zcode"]);
        assert_eq!(daily.rows.len(), 3);
        assert_eq!(daily.rows[0].period, "2099-01-02");
        assert_eq!(daily.rows[0].input_tokens, 60);
        assert_eq!(daily.rows[0].output_tokens, 10);
        assert_eq!(daily.rows[0].cache_creation_tokens, 15);
        assert_eq!(daily.rows[0].cache_read_tokens, 25);
        assert_eq!(daily.rows[0].total_tokens, 110);
        assert_eq!(daily.rows[0].total_cost, 0.00015549999999999999);
        assert_eq!(daily.rows[1].period, "2099-01-15");
        assert_eq!(daily.rows[1].input_tokens, 130);
        assert_eq!(daily.rows[1].output_tokens, 20);
        assert_eq!(daily.rows[1].cache_creation_tokens, 30);
        assert_eq!(daily.rows[1].cache_read_tokens, 40);
        assert_eq!(daily.rows[1].total_tokens, 220);
        assert_eq!(daily.rows[1].total_cost, 0.0003224);
        assert_eq!(daily.rows[2].period, "2099-02-01");
        assert_eq!(daily.rows[2].input_tokens, 40);
        assert_eq!(daily.rows[2].output_tokens, 5);
        assert_eq!(daily.rows[2].cache_creation_tokens, 0);
        assert_eq!(daily.rows[2].cache_read_tokens, 10);
        assert_eq!(daily.rows[2].total_tokens, 55);
        assert_eq!(daily.rows[2].total_cost, 0.0000806);
        assert_eq!(monthly.rows[0].period, "2099-01");
        assert_eq!(monthly.rows[0].total_tokens, 330);
        assert_eq!(monthly.rows[0].total_cost, 0.00047789999999999996);
        assert_eq!(monthly.rows[1].period, "2099-02");
        assert_eq!(monthly.rows[1].total_tokens, 55);
        assert_eq!(monthly.rows[1].total_cost, 0.0000806);
        assert_eq!(session.rows[0].period, "session-a");
        assert_eq!(session.rows[0].total_tokens, 330);
        assert_eq!(session.rows[0].total_cost, 0.00047789999999999996);
        assert_eq!(session.rows[1].period, "session-b");
        assert_eq!(session.rows[1].total_tokens, 55);
        assert_eq!(session.rows[1].total_cost, 0.0000806);

        insta::assert_json_snapshot!(
            "zcode_fixture_daily_json",
            report_json_with_agents(&daily.rows, AgentReportKind::Daily, false)
        );
        insta::assert_json_snapshot!(
            "zcode_fixture_monthly_json",
            report_json_with_agents(&monthly.rows, AgentReportKind::Monthly, false)
        );
        insta::assert_json_snapshot!(
            "zcode_fixture_session_json",
            report_json_with_agents(&session.rows, AgentReportKind::Session, false)
        );
        insta::assert_snapshot!(
            "zcode_fixture_daily_table",
            serde_json::to_string_pretty(&table_snapshot(
                &daily.rows,
                AgentReportKind::Daily,
                &daily.detected_agents,
            ))
            .unwrap()
        );
        insta::assert_snapshot!(
            "zcode_fixture_monthly_table",
            serde_json::to_string_pretty(&table_snapshot(
                &monthly.rows,
                AgentReportKind::Monthly,
                &monthly.detected_agents,
            ))
            .unwrap()
        );
        insta::assert_snapshot!(
            "zcode_fixture_session_table",
            serde_json::to_string_pretty(&table_snapshot(
                &session.rows,
                AgentReportKind::Session,
                &session.detected_agents,
            ))
            .unwrap()
        );
    })
}

#[test]
fn cline_fixture_reports_all_kinds_through_unified_json_and_tables() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "cline/tasks/session-a/ui_messages.json": include_str!(
            "../../../../adapters/cline/tests/fixtures/extension/ui_messages.json"
        ),
        "cline/tasks/session-a/task_metadata.json": include_str!(
            "../../../../adapters/cline/tests/fixtures/extension/task_metadata.json"
        ),
        "cline/tasks/session-a/api_conversation_history.json": include_str!(
            "../../../../adapters/cline/tests/fixtures/extension/api_conversation_history.json"
        ),
        "cline/data/sessions/session-a/session-a.messages.json": include_str!(
            "../../../../adapters/cline/tests/fixtures/cli/session.messages.json"
        ),
    });
    fixture.write_file(
        "cline/data/sessions/rejected-cache/rejected-cache.messages.json",
        r#"{"version":1,"sessionId":"rejected-cache","messages":[{"role":"assistant","ts":1793491260000,"id":"cache-exceeds-input-session","modelInfo":{"id":"claude-sonnet-4-20250514","provider":"anthropic"},"metrics":{"inputTokens":5,"outputTokens":2,"cacheReadTokens":4,"cacheWriteTokens":2,"cost":0.5}}]}"#,
    );
    isolated_agent_env(
        &fixture,
        "CLINE_DATA_DIR",
        fixture.path("cline").into_os_string(),
    )
    .run(|| {
        let mut shared = fixture_shared("20261001", "20261101");
        shared.mode = CostMode::Auto;

        for kind in [
            AgentReportKind::Daily,
            AgentReportKind::Weekly,
            AgentReportKind::Monthly,
            AgentReportKind::Session,
        ] {
            let result = load_rows(kind, &shared, &NO_SOURCES, WeekDay::Monday).unwrap();

            assert_eq!(result.detected_agents, ["cline"]);
            assert!(
                !result.rows.is_empty(),
                "{kind:?} should include Cline rows"
            );
            assert_cline_unified_result_contains_only_accepted_requests(&result.rows, kind);
            if kind == AgentReportKind::Session {
                assert!(result.rows.iter().all(|row| row.agent() == "cline"));
                assert!(
                    result
                        .rows
                        .iter()
                        .all(|row| row.agents().map(<[_]>::to_vec).is_none())
                );
            } else {
                assert!(result.rows.iter().all(|row| row.agent() == "all"));
                assert!(
                    result
                        .rows
                        .iter()
                        .all(|row| row.agents().map(<[_]>::to_vec).as_deref()
                            == Some(["cline"].as_slice()))
                );
            }

            insta::assert_json_snapshot!(
                format!("cline_fixture_{kind:?}_unified_json").to_lowercase(),
                report_json_with_agents(&result.rows, kind, false)
            );
            insta::assert_snapshot!(
                format!("cline_fixture_{kind:?}_unified_table").to_lowercase(),
                serde_json::to_string_pretty(&table_snapshot(
                    &result.rows,
                    kind,
                    &result.detected_agents,
                ))
                .unwrap()
            );
        }
    })
}

fn assert_cline_unified_result_contains_only_accepted_requests(
    rows: &[ReportRow],
    kind: AgentReportKind,
) {
    for (actual, expected, label) in [
        (
            rows.iter().map(|row| row.input_tokens).sum::<u64>(),
            255,
            "input tokens",
        ),
        (
            rows.iter().map(|row| row.output_tokens).sum::<u64>(),
            51,
            "output tokens",
        ),
        (
            rows.iter()
                .map(|row| row.cache_creation_tokens)
                .sum::<u64>(),
            30,
            "cache creation tokens",
        ),
        (
            rows.iter().map(|row| row.cache_read_tokens).sum::<u64>(),
            50,
            "cache read tokens",
        ),
        (
            rows.iter().map(|row| row.total_tokens).sum::<u64>(),
            386,
            "total tokens",
        ),
    ] {
        assert_eq!(actual, expected, "{kind:?} {label}");
    }
    let total_cost = rows.iter().map(|row| row.total_cost).sum::<f64>();
    assert!((total_cost - 0.0012).abs() < 1e-12, "{kind:?} cost");

    let mut periods = rows
        .iter()
        .map(|row| row.period.as_str())
        .collect::<Vec<_>>();
    periods.sort_unstable();
    let expected_periods: &[&str] = match kind {
        AgentReportKind::Daily => &["2026-10-01", "2026-11-01"],
        AgentReportKind::Weekly => &["2026-09-28", "2026-10-26"],
        AgentReportKind::Monthly => &["2026-10", "2026-11"],
        AgentReportKind::Session => &["cli:session-a", "task:session-a"],
    };
    assert_eq!(periods, expected_periods.to_vec(), "{kind:?} periods");

    let mut models = rows
        .iter()
        .flat_map(|row| row.models_used.iter().map(String::as_str))
        .collect::<Vec<_>>();
    models.sort_unstable();
    models.dedup();
    assert_eq!(models, ["claude-sonnet-4-20250514"], "{kind:?} models");
}

#[test]
fn unified_report_omits_zcode_without_usage_database() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    isolated_agent_env(
        &fixture,
        "ZCODE_HOME",
        fixture.path("missing-zcode").into_os_string(),
    )
    .run(|| {
        let shared = fixture_shared("20990101", "20990201");

        let result = load_rows(
            AgentReportKind::Daily,
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        )
        .unwrap();

        assert!(result.rows.is_empty());
        assert!(result.detected_agents.is_empty());
    })
}

#[test]
fn unified_report_filters_codex_paths_before_loading_historical_rows() {
    ccusage_test_support::install_pricing_snapshots();
    let resumed_usage = codex_usage_line("2026-03-15T08:01:00.000Z", "gpt-5", 1_000);
    let historical_usage = codex_usage_line("2026-03-15T08:02:00.000Z", "gpt-5", 9_999);
    let fixture = fs_fixture!({
        "codex/sessions/2025/01/01/resumed.jsonl": &resumed_usage,
        "codex/sessions/2025/01/02/historical.jsonl": &historical_usage,
    });
    set_modified(
        &fixture.path("codex/sessions/2025/01/01/resumed.jsonl"),
        instant("2026-03-15T08:01:00.000Z"),
    );
    set_modified(
        &fixture.path("codex/sessions/2025/01/02/historical.jsonl"),
        instant("2025-01-02T08:01:00.000Z"),
    );
    isolated_agent_env(
        &fixture,
        "CODEX_HOME",
        fixture.path("codex").into_os_string(),
    )
    .run(|| {
        let shared = fixture_shared("20260315", "20260315");

        let result = load_rows(
            AgentReportKind::Daily,
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        )
        .unwrap();

        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].period, "2026-03-15");
        assert_eq!(result.rows[0].total_tokens, 1_300);
        assert_eq!(result.detected_agents, vec!["codex"]);
    })
}

fn assert_unified_rows_use_grok_home(kind: AgentReportKind) {
    let line = r#"{"timestamp":1750000000,"params":{"sessionId":"sess-grok","update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":100,"outputTokens":20,"cachedReadTokens":40,"reasoningTokens":10,"modelUsage":{"grok-4.5-build":{"inputTokens":100,"outputTokens":20,"cachedReadTokens":40,"reasoningTokens":10}}}},"_meta":{"eventId":"evt-grok"}}}"#;
    let fixture = fs_fixture!({
        "grok/sessions/proj/sess-grok/updates.jsonl": line,
    });
    isolated_agent_env(&fixture, "GROK_HOME", fixture.path("grok").into_os_string()).run(|| {
        let shared = fixture_shared("20250615", "20250615");

        let result = loader::load_rows(kind, &shared, &NO_SOURCES, WeekDay::Monday).unwrap();

        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.detected_agents, vec!["grok"]);
        assert_eq!(
            result.rows[0].agents().map(<[_]>::to_vec),
            (kind != AgentReportKind::Session).then_some(vec!["grok"])
        );
        assert_eq!(result.rows[0].input_tokens, 60);
        assert_eq!(result.rows[0].cache_read_tokens, 40);
        assert_eq!(result.rows[0].output_tokens, 20);
    })
}

#[test]
fn unified_daily_rows_use_grok_home() {
    ccusage_test_support::install_pricing_snapshots();
    assert_unified_rows_use_grok_home(AgentReportKind::Daily);
}

#[test]
fn unified_session_rows_use_grok_home() {
    ccusage_test_support::install_pricing_snapshots();
    assert_unified_rows_use_grok_home(AgentReportKind::Session);
}

#[test]
fn unified_weekly_rows_and_sections_honor_start_of_week() {
    ccusage_test_support::install_pricing_snapshots();
    let line = r#"{"timestamp":1750000000,"params":{"sessionId":"sess-grok","update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":100,"outputTokens":20,"modelUsage":{"grok-4.5-build":{"inputTokens":100,"outputTokens":20}}}},"_meta":{"eventId":"evt-grok"}}}"#;
    let fixture = fs_fixture!({
        "grok/sessions/proj/sess-grok/updates.jsonl": line,
    });
    isolated_agent_env(&fixture, "GROK_HOME", fixture.path("grok").into_os_string()).run(|| {
        let shared = fixture_shared("20250615", "20250615");

        for (start_of_week, week) in [
            (WeekDay::Sunday, "2025-06-15"),
            (WeekDay::Monday, "2025-06-09"),
        ] {
            let rows =
                load_rows(AgentReportKind::Weekly, &shared, &NO_SOURCES, start_of_week).unwrap();
            let periods: Vec<_> = rows.rows.iter().map(|row| row.period.as_str()).collect();
            assert_eq!(periods, [week], "{start_of_week:?} load_rows");

            let sections = load_sections(
                &[AgentReportKind::Daily, AgentReportKind::Weekly],
                &shared,
                &NO_SOURCES,
                start_of_week,
            )
            .unwrap();
            let (_, weekly) = sections
                .sections
                .iter()
                .find(|(kind, _)| *kind == AgentReportKind::Weekly)
                .unwrap();
            let periods: Vec<_> = weekly.iter().map(|row| row.period.as_str()).collect();
            assert_eq!(periods, [week], "{start_of_week:?} load_sections");
        }
    })
}

fn codex_usage_line(timestamp: &str, model: &str, input_tokens: u64) -> String {
    json!({
        "timestamp": timestamp,
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": {
                "model": model,
                "last_token_usage": {
                    "input_tokens": input_tokens,
                    "cached_input_tokens": 100,
                    "output_tokens": 200,
                    "reasoning_output_tokens": 20,
                    "total_tokens": input_tokens + 300,
                },
            },
        },
    })
    .to_string()
}

#[test]
fn dsh_fixture_reports_all_kinds_through_unified_json_and_tables() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "dsh/sessions/--home-user-project--/dsh-parent/session.v4.jsonl": include_str!(
            "../../../../adapters/dsh/tests/fixtures/parent/session.v4.jsonl"
        ),
        "dsh/sessions/--home-user-project--/dsh-fork/session.v4.jsonl": include_str!(
            "../../../../adapters/dsh/tests/fixtures/fork/session.v4.jsonl"
        ),
    });
    isolated_agent_env(&fixture, "DSH_HOME", fixture.path("dsh").into_os_string()).run(|| {
        let shared = fixture_shared("20261001", "20261101");

        for kind in [
            AgentReportKind::Daily,
            AgentReportKind::Weekly,
            AgentReportKind::Monthly,
            AgentReportKind::Session,
        ] {
            let result = load_rows(kind, &shared, &NO_SOURCES, WeekDay::Monday).unwrap();

            assert_eq!(result.detected_agents, ["dsh"]);
            insta::assert_json_snapshot!(
                format!("dsh_fixture_{kind:?}_unified_json").to_lowercase(),
                report_json_with_agents(&result.rows, kind, false)
            );
            insta::assert_snapshot!(
                format!("dsh_fixture_{kind:?}_unified_table").to_lowercase(),
                serde_json::to_string_pretty(&table_snapshot(
                    &result.rows,
                    kind,
                    &result.detected_agents,
                ))
                .unwrap()
            );
        }
    })
}
