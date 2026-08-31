use std::{
    ffi::OsString,
    fs::{File, FileTimes},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant, UNIX_EPOCH},
};

use serde_json::{Value, json};

use super::*;
use crate::{
    Align, CodexGroup, CodexModelUsage, ModelBreakdown, PricingMap,
    cli::{AgentReportKind, CodexSpeed, CostMode, SharedArgs},
    model_aliases::set_model_aliases_for_tests,
};
use ccusage_adapter_codex::CodexSourceUsage;
use ccusage_test_support::{EnvVarsGuard, fs_fixture, zcode::create_fixture};

fn test_agent_rows(agent: &'static str) -> AgentRows {
    AgentRows {
        rows: vec![AllRow {
            period: "2026-01-02".to_string(),
            agent,
            models_used: Vec::new(),
            input_tokens: 1,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            total_tokens: 1,
            total_cost: 0.0,
            metadata: None,
            metadata_agents: Some(vec![agent]),
            agent_breakdowns: None,
            model_breakdowns: Vec::new(),
        }],
        detected: true,
    }
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
    let rows = aggregate_rows(
        vec![
            AllRow {
                period: "2026-01-02".to_string(),
                agent: "codex",
                models_used: vec!["gpt-5".to_string()],
                input_tokens: 100,
                output_tokens: 20,
                cache_creation_tokens: 0,
                cache_read_tokens: 10,
                total_tokens: 120,
                total_cost: 0.01,
                metadata: None,
                metadata_agents: Some(vec!["codex"]),
                agent_breakdowns: None,
                model_breakdowns: Vec::new(),
            },
            AllRow {
                period: "2026-01-02".to_string(),
                agent: "claude",
                models_used: vec!["claude-sonnet-4-20250514".to_string()],
                input_tokens: 50,
                output_tokens: 25,
                cache_creation_tokens: 5,
                cache_read_tokens: 3,
                total_tokens: 83,
                total_cost: 0.02,
                metadata: None,
                metadata_agents: Some(vec!["claude"]),
                agent_breakdowns: None,
                model_breakdowns: Vec::new(),
            },
        ],
        AgentReportKind::Daily,
    );

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].period, "2026-01-02");
    assert_eq!(rows[0].agent, "all");
    assert_eq!(rows[0].input_tokens, 150);
    assert_eq!(rows[0].output_tokens, 45);
    assert_eq!(rows[0].cache_read_tokens, 13);
    assert_eq!(rows[0].total_tokens, 203);
    assert_eq!(
        rows[0].models_used,
        vec!["claude-sonnet-4-20250514".to_string(), "gpt-5".to_string()]
    );
    assert_eq!(rows[0].metadata_agents, Some(vec!["claude", "codex"]));
    let breakdowns = rows[0].agent_breakdowns.as_ref().unwrap();
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
    let daily_rows = aggregate_rows(rows.clone(), AgentReportKind::Daily);
    let daily_report = report_json_with_options(&daily_rows, AgentReportKind::Daily, false, true);
    let weekly_rows = aggregate_rows(rows, AgentReportKind::Weekly);
    let weekly_report =
        report_json_with_options(&weekly_rows, AgentReportKind::Weekly, false, true);

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
        assert_eq!(
            report["totals"]["sourceBreakdowns"][0]["inputTokens"],
            u64::MAX
        );
        assert_eq!(
            report["totals"]["sourceBreakdowns"][0]["modelBreakdowns"][0]["inputTokens"],
            u64::MAX
        );
    }
    assert_eq!(weekly_rows[0].model_breakdowns[0].input_tokens, u64::MAX);
    assert_eq!(weekly_rows[0].model_breakdowns[0].output_tokens, u64::MAX);
}

fn extreme_all_row(period: &str, tokens: u64) -> AllRow {
    AllRow {
        period: period.to_string(),
        agent: "codex",
        models_used: vec!["gpt-5".to_string()],
        input_tokens: tokens,
        output_tokens: tokens,
        cache_creation_tokens: tokens,
        cache_read_tokens: tokens,
        total_tokens: tokens,
        total_cost: 0.0,
        metadata: Some(json!({
            "sourceBreakdowns": [{
                "source": "Responses API",
                "modelsUsed": ["gpt-5"],
                "inputTokens": tokens,
                "outputTokens": tokens,
                "cacheCreationTokens": tokens,
                "cacheReadTokens": tokens,
                "reasoningOutputTokens": tokens,
                "totalTokens": tokens,
                "totalCost": 0.0,
                "modelBreakdowns": [{
                    "modelName": "gpt-5",
                    "inputTokens": tokens,
                    "outputTokens": tokens,
                    "cacheCreationTokens": tokens,
                    "cacheReadTokens": tokens,
                    "cost": 0.0
                }]
            }]
        })),
        metadata_agents: Some(vec!["codex"]),
        agent_breakdowns: None,
        model_breakdowns: vec![ModelBreakdown {
            model_name: "gpt-5".to_string(),
            input_tokens: tokens,
            output_tokens: tokens,
            cache_creation_tokens: tokens,
            cache_read_tokens: tokens,
            extra_total_tokens: tokens,
            ..ModelBreakdown::default()
        }],
    }
}

#[test]
fn merges_same_agent_daily_rows_into_one_monthly_breakdown() {
    let rows = aggregate_rows(
        vec![
            AllRow {
                period: "2026-01-02".to_string(),
                agent: "claude",
                models_used: vec!["claude-sonnet-4-20250514".to_string()],
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_tokens: 1,
                cache_read_tokens: 2,
                total_tokens: 18,
                total_cost: 0.01,
                metadata: None,
                metadata_agents: Some(vec!["claude"]),
                agent_breakdowns: None,
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
            AllRow {
                period: "2026-01-15".to_string(),
                agent: "claude",
                models_used: vec!["claude-opus-4-20250514".to_string()],
                input_tokens: 20,
                output_tokens: 10,
                cache_creation_tokens: 2,
                cache_read_tokens: 4,
                total_tokens: 36,
                total_cost: 0.05,
                metadata: None,
                metadata_agents: Some(vec!["claude"]),
                agent_breakdowns: None,
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
            AllRow {
                period: "2026-01-20".to_string(),
                agent: "codex",
                models_used: vec!["gpt-5".to_string()],
                input_tokens: 30,
                output_tokens: 15,
                cache_creation_tokens: 0,
                cache_read_tokens: 6,
                total_tokens: 51,
                total_cost: 0.02,
                metadata: None,
                metadata_agents: Some(vec!["codex"]),
                agent_breakdowns: None,
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
        ],
        AgentReportKind::Monthly,
    );

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].period, "2026-01");
    assert_eq!(rows[0].input_tokens, 60);
    assert_eq!(rows[0].output_tokens, 30);
    let breakdowns = rows[0].agent_breakdowns.as_ref().unwrap();
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

#[test]
fn renders_all_report_json_with_period_and_agent_metadata() {
    let rows = vec![AllRow {
        period: "2026-01-02".to_string(),
        agent: "all",
        models_used: vec!["gpt-5".to_string()],
        input_tokens: 100,
        output_tokens: 20,
        cache_creation_tokens: 0,
        cache_read_tokens: 10,
        total_tokens: 130,
        total_cost: 0.01,
        metadata: None,
        metadata_agents: Some(vec!["codex"]),
        agent_breakdowns: None,
        model_breakdowns: Vec::new(),
    }];

    let report = report_json(&rows, AgentReportKind::Daily);

    assert_eq!(report["daily"][0]["period"], "2026-01-02");
    assert_eq!(report["daily"][0]["agent"], "all");
    assert_eq!(report["daily"][0]["metadata"]["agents"], json!(["codex"]));
    assert_eq!(report["totals"]["totalTokens"], 130);
}

#[test]
fn renders_opt_in_codex_source_breakdowns_in_unified_json_and_table_rows() {
    let model_usage = |input_tokens: u64, output_tokens: u64, total_tokens: u64| CodexModelUsage {
        input_tokens,
        output_tokens,
        total_tokens,
        ..CodexModelUsage::default()
    };
    let cli_usage = CodexSourceUsage {
        input_tokens: 100,
        output_tokens: 10,
        total_tokens: 110,
        models: std::collections::BTreeMap::from([(
            "gpt-5".to_string(),
            model_usage(100, 10, 110),
        )]),
        ..CodexSourceUsage::default()
    };
    let exec_usage = CodexSourceUsage {
        input_tokens: 50,
        output_tokens: 5,
        total_tokens: 55,
        models: std::collections::BTreeMap::from([("gpt-5".to_string(), model_usage(50, 5, 55))]),
        ..CodexSourceUsage::default()
    };
    let group = CodexGroup {
        input_tokens: 150,
        output_tokens: 15,
        total_tokens: 165,
        models: std::collections::BTreeMap::from([(
            "gpt-5".to_string(),
            model_usage(150, 15, 165),
        )]),
        sources: std::collections::BTreeMap::from([
            ("CLI".to_string(), cli_usage),
            ("Exec".to_string(), exec_usage),
        ]),
        ..CodexGroup::default()
    };
    let row = codex_group_row(
        "2026-01-02",
        &group,
        &PricingMap::default(),
        CodexSpeed::Standard,
    );
    let default = report_json(std::slice::from_ref(&row), AgentReportKind::Daily);
    let report = report_json_with_options(&[row], AgentReportKind::Daily, false, true);

    assert!(default["daily"][0].get("sourceBreakdowns").is_none());
    let sources = report["daily"][0]["sourceBreakdowns"].as_array().unwrap();
    assert_eq!(sources.len(), 2);
    assert_eq!(sources[0]["source"], "CLI");
    assert_eq!(sources[1]["source"], "Exec");
    assert_eq!(
        sources
            .iter()
            .map(|source| source["totalTokens"].as_u64().unwrap())
            .sum::<u64>(),
        report["daily"][0]["totalTokens"].as_u64().unwrap()
    );
    let monthly_rows = aggregate_rows(
        vec![
            codex_group_row(
                "2026-01-02",
                &group,
                &PricingMap::default(),
                CodexSpeed::Standard,
            ),
            codex_group_row(
                "2026-01-03",
                &group,
                &PricingMap::default(),
                CodexSpeed::Standard,
            ),
        ],
        AgentReportKind::Monthly,
    );
    let monthly_report =
        report_json_with_options(&monthly_rows, AgentReportKind::Monthly, false, true);
    let monthly_sources = monthly_report["monthly"][0]["sourceBreakdowns"]
        .as_array()
        .unwrap();
    assert_eq!(monthly_sources[0]["totalTokens"], 220);
    assert_eq!(monthly_sources[1]["totalTokens"], 110);
    assert_eq!(monthly_report["monthly"][0]["totalTokens"], 330);
    insta::assert_json_snapshot!(report);
    insta::assert_debug_snapshot!(source_table_row(&sources[0], false, false));
}

#[test]
fn sanitizes_unknown_codex_source_only_in_unified_table_rows() {
    let source = "future\nclient\u{1b}[31m";
    let value = json!({
        "source": source,
        "modelsUsed": [],
        "inputTokens": 0,
        "outputTokens": 0,
        "cacheCreationTokens": 0,
        "cacheReadTokens": 0,
        "totalCost": 0.0,
    });

    assert_eq!(
        source_table_row(&value, false, false)[1],
        r#"- future\nclient\u{1b}[31m"#
    );
    let row = AllRow {
        period: "2026-08-20".to_string(),
        agent: "codex",
        models_used: Vec::new(),
        input_tokens: 0,
        output_tokens: 0,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        total_tokens: 0,
        total_cost: 0.0,
        metadata: Some(json!({ "sourceBreakdowns": [value] })),
        metadata_agents: Some(vec!["codex"]),
        agent_breakdowns: None,
        model_breakdowns: Vec::new(),
    };
    let report = report_json_with_options(&[row], AgentReportKind::Daily, false, true);

    assert_eq!(report["daily"][0]["sourceBreakdowns"][0]["source"], source);
}

#[test]
fn renders_by_agent_json_breakdowns_when_requested() {
    let rows = vec![AllRow {
        period: "2026-01-02".to_string(),
        agent: "all",
        models_used: vec!["claude-sonnet-4-20250514".to_string(), "gpt-5".to_string()],
        input_tokens: 150,
        output_tokens: 45,
        cache_creation_tokens: 5,
        cache_read_tokens: 13,
        total_tokens: 203,
        total_cost: 0.03,
        metadata: None,
        metadata_agents: Some(vec!["claude", "codex"]),
        agent_breakdowns: Some(vec![
            AllRow {
                period: "2026-01-02".to_string(),
                agent: "claude",
                models_used: vec!["claude-sonnet-4-20250514".to_string()],
                input_tokens: 50,
                output_tokens: 25,
                cache_creation_tokens: 5,
                cache_read_tokens: 3,
                total_tokens: 83,
                total_cost: 0.02,
                metadata: None,
                metadata_agents: Some(vec!["claude"]),
                agent_breakdowns: None,
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
            AllRow {
                period: "2026-01-02".to_string(),
                agent: "codex",
                models_used: vec!["gpt-5".to_string()],
                input_tokens: 100,
                output_tokens: 20,
                cache_creation_tokens: 0,
                cache_read_tokens: 10,
                total_tokens: 120,
                total_cost: 0.01,
                metadata: None,
                metadata_agents: Some(vec!["codex"]),
                agent_breakdowns: None,
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
        ]),
        model_breakdowns: Vec::new(),
    }];

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
    let rows = aggregate_rows(
        vec![AllRow {
            period: "2026-01-02".to_string(),
            agent: "codex",
            models_used: vec!["gpt-5".to_string()],
            input_tokens: 100,
            output_tokens: 20,
            cache_creation_tokens: 0,
            cache_read_tokens: 10,
            total_tokens: 120,
            total_cost: 0.01,
            metadata: None,
            metadata_agents: Some(vec!["codex"]),
            agent_breakdowns: None,
            model_breakdowns: Vec::new(),
        }],
        AgentReportKind::Daily,
    );

    let report = report_json(&rows, AgentReportKind::Daily);

    assert!(report["daily"][0].get("agents").is_none());
}

#[test]
fn renders_multi_section_json_with_command_totals() {
    let daily_rows = vec![AllRow {
        period: "2026-01-02".to_string(),
        agent: "all",
        models_used: vec!["gpt-5".to_string()],
        input_tokens: 100,
        output_tokens: 20,
        cache_creation_tokens: 0,
        cache_read_tokens: 10,
        total_tokens: 120,
        total_cost: 0.01,
        metadata: None,
        metadata_agents: Some(vec!["codex"]),
        agent_breakdowns: None,
        model_breakdowns: Vec::new(),
    }];
    let monthly_rows = aggregate_rows(daily_rows.clone(), AgentReportKind::Monthly);
    let session_rows = vec![AllRow {
        period: "session-a".to_string(),
        agent: "codex",
        models_used: vec!["gpt-5".to_string()],
        input_tokens: 100,
        output_tokens: 20,
        cache_creation_tokens: 0,
        cache_read_tokens: 10,
        total_tokens: 120,
        total_cost: 0.01,
        metadata: Some(json!({ "lastActivity": "2026-01-02T00:00:00.000Z" })),
        metadata_agents: None,
        agent_breakdowns: None,
        model_breakdowns: Vec::new(),
    }];
    let sections = vec![
        (AgentReportKind::Daily, daily_rows.clone()),
        (AgentReportKind::Monthly, monthly_rows.clone()),
        (AgentReportKind::Session, session_rows.clone()),
    ];

    let report = sections_report_json(&sections, AgentReportKind::Daily, false);

    assert_eq!(
        report.get("daily").unwrap(),
        &report_json(&daily_rows, AgentReportKind::Daily)["daily"]
    );
    assert_eq!(
        report.get("monthly").unwrap(),
        &report_json(&monthly_rows, AgentReportKind::Monthly)["monthly"]
    );
    assert_eq!(
        report.get("session").unwrap(),
        &report_json(&session_rows, AgentReportKind::Session)["session"]
    );
    assert_eq!(
        report.get("totals").unwrap(),
        &report_json(&daily_rows, AgentReportKind::Daily)["totals"]
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
fn multi_section_claude_fixture_matches_standalone_sections_for_daily_and_session_invocations() {
    let fixture = fs_fixture!({
        "projects/project-a/session-a.jsonl": [
            r#"{"timestamp":"2099-01-02T00:00:00.000Z","sessionId":"session-a","requestId":"req-direct","costUSD":0.01,"message":{"usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":0,"cache_read_input_tokens":0},"model":"claude-sonnet-4-20250514","id":"msg-direct"}}"#,
            r#"{"data":{"message":{"timestamp":"2099-01-02T00:01:00.000Z","requestId":"req-progress","isSidechain":true,"costUSD":0.02,"message":{"usage":{"input_tokens":1000,"output_tokens":500,"cache_creation_input_tokens":0,"cache_read_input_tokens":0},"model":"claude-sonnet-4-20250514","id":"msg-progress"}}}}"#,
        ]
        .join("\n"),
    });
    let _env = isolated_agent_env(
        &fixture,
        "CLAUDE_CONFIG_DIR",
        fixture.root().as_os_str().into(),
    );
    let shared = fixture_shared("20990102", "20990102");

    assert_daily_family_and_session_sections_match_standalone(&shared);
}

#[test]
fn multi_section_codex_fixture_matches_standalone_sections_for_daily_and_session_invocations() {
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
    let _env = isolated_agent_env(
        &fixture,
        "CODEX_HOME",
        fixture.path("codex").into_os_string(),
    );
    let shared = fixture_shared("20990201", "20990202");

    assert_daily_family_and_session_sections_match_standalone(&shared);
}

#[test]
fn zcode_fixture_reports_daily_monthly_session_json_and_table_snapshots() {
    let fixture = fs_fixture!({});
    let _ = fixture.create_dir_all("zcode/cli/db");
    let db_path = fixture.path("zcode/cli/db/db.sqlite");
    create_fixture(db_path);
    let _env = isolated_agent_env(
        &fixture,
        "ZCODE_HOME",
        fixture.path("zcode").into_os_string(),
    );
    let mut shared = fixture_shared("20990101", "20990201");
    shared.mode = CostMode::Calculate;

    let daily = load_rows(AgentReportKind::Daily, &shared).unwrap();
    let monthly = load_rows(AgentReportKind::Monthly, &shared).unwrap();
    let session = load_rows(AgentReportKind::Session, &shared).unwrap();

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
        report_json(&daily.rows, AgentReportKind::Daily)
    );
    insta::assert_json_snapshot!(
        "zcode_fixture_monthly_json",
        report_json(&monthly.rows, AgentReportKind::Monthly)
    );
    insta::assert_json_snapshot!(
        "zcode_fixture_session_json",
        report_json(&session.rows, AgentReportKind::Session)
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
}

#[test]
fn unified_report_omits_zcode_without_usage_database() {
    let fixture = fs_fixture!({});
    let _env = isolated_agent_env(
        &fixture,
        "ZCODE_HOME",
        fixture.path("missing-zcode").into_os_string(),
    );
    let shared = fixture_shared("20990101", "20990201");

    let result = load_rows(AgentReportKind::Daily, &shared).unwrap();

    assert!(result.rows.is_empty());
    assert!(result.detected_agents.is_empty());
}

#[test]
fn unified_report_filters_codex_paths_before_loading_historical_rows() {
    let resumed_usage = codex_usage_line("2026-03-15T08:01:00.000Z", "gpt-5", 1_000);
    let historical_usage = codex_usage_line("2026-03-15T08:02:00.000Z", "gpt-5", 9_999);
    let fixture = fs_fixture!({
        "codex/sessions/2025/01/01/resumed.jsonl": &resumed_usage,
        "codex/sessions/2025/01/02/historical.jsonl": &historical_usage,
    });
    set_file_modified(
        &fixture.path("codex/sessions/2025/01/01/resumed.jsonl"),
        "2026-03-15T08:01:00.000Z",
    );
    set_file_modified(
        &fixture.path("codex/sessions/2025/01/02/historical.jsonl"),
        "2025-01-02T08:01:00.000Z",
    );
    let _env = isolated_agent_env(
        &fixture,
        "CODEX_HOME",
        fixture.path("codex").into_os_string(),
    );
    let shared = fixture_shared("20260315", "20260315");

    let result = load_rows(AgentReportKind::Daily, &shared).unwrap();

    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].period, "2026-03-15");
    assert_eq!(result.rows[0].total_tokens, 1_300);
    assert_eq!(result.detected_agents, vec!["codex"]);
}

fn set_file_modified(path: &Path, timestamp: &str) {
    let milliseconds = u64::try_from(parse_ts_timestamp(timestamp).unwrap().as_millis()).unwrap();
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_millis(milliseconds)))
        .unwrap();
}

fn fixture_shared(since: &str, until: &str) -> SharedArgs {
    SharedArgs {
        since: Some(since.to_string()),
        until: Some(until.to_string()),
        timezone: Some("UTC".to_string()),
        offline: true,
        single_thread: true,
        ..SharedArgs::default()
    }
}

fn table_snapshot(
    rows: &[AllRow],
    kind: AgentReportKind,
    detected_agents: &[&'static str],
) -> Value {
    let (headers, _) = all_table_columns(kind, false, false);
    let mut rendered_rows = Vec::new();
    for row in rows {
        rendered_rows.push(json!({
            "kind": "row",
            "cells": all_table_row(row, false, false, false),
        }));
        if let Some(agent_breakdowns) = row.agent_breakdowns.as_ref() {
            for breakdown in agent_breakdowns {
                rendered_rows.push(json!({
                    "kind": "agent-breakdown",
                    "cells": all_table_row(breakdown, false, true, false),
                }));
            }
        }
    }
    let totals = report_json(rows, kind)["totals"].clone();
    let total_row = AllRow {
        period: "Total".to_string(),
        agent: "all",
        models_used: Vec::new(),
        input_tokens: totals["inputTokens"].as_u64().unwrap(),
        output_tokens: totals["outputTokens"].as_u64().unwrap(),
        cache_creation_tokens: totals["cacheCreationTokens"].as_u64().unwrap(),
        cache_read_tokens: totals["cacheReadTokens"].as_u64().unwrap(),
        total_tokens: totals["totalTokens"].as_u64().unwrap(),
        total_cost: totals["totalCost"].as_f64().unwrap(),
        metadata: None,
        metadata_agents: None,
        agent_breakdowns: None,
        model_breakdowns: Vec::new(),
    };
    let mut total_cells = all_table_row(&total_row, false, false, false);
    total_cells[1].clear();
    total_cells[2].clear();
    rendered_rows.push(json!({
        "kind": "total",
        "cells": total_cells,
    }));

    json!({
        "title": all_report_title(kind, rows, detected_agents),
        "headers": headers,
        "rows": rendered_rows,
    })
}

fn isolated_agent_env(
    fixture: &ccusage_test_support::Fixture,
    source_key: &'static str,
    source_value: OsString,
) -> EnvVarsGuard {
    let home = fixture.path("empty-home").into_os_string();
    let xdg_config = fixture.path("empty-xdg-config").into_os_string();
    let mut vars = [
        "CLAUDE_CONFIG_DIR",
        "CODEX_HOME",
        "OPENCODE_DATA_DIR",
        "AMP_DATA_DIR",
        "DROID_SESSIONS_DIR",
        "CODEBUFF_DATA_DIR",
        "HERMES_HOME",
        "PI_AGENT_DIR",
        "GOOSE_PATH_ROOT",
        "OPENCLAW_DIR",
        "KILO_DATA_DIR",
        "COPILOT_OTEL_FILE_EXPORTER_PATH",
        "GEMINI_DATA_DIR",
        "KIMI_DATA_DIR",
        "QWEN_DATA_DIR",
        "GROK_HOME",
        "ZCODE_HOME",
    ]
    .into_iter()
    .map(|key| (key, None::<OsString>))
    .collect::<Vec<_>>();
    vars.push(("HOME", Some(home)));
    vars.push((
        "USERPROFILE",
        Some(fixture.path("empty-userprofile").into_os_string()),
    ));
    vars.push(("XDG_CONFIG_HOME", Some(xdg_config)));
    vars.push((source_key, Some(source_value)));
    EnvVarsGuard::set_many(vars)
}

fn assert_unified_rows_use_grok_home(kind: AgentReportKind) {
    let line = r#"{"timestamp":1750000000,"params":{"sessionId":"sess-grok","update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":100,"outputTokens":20,"cachedReadTokens":40,"reasoningTokens":10,"modelUsage":{"grok-4.5-build":{"inputTokens":100,"outputTokens":20,"cachedReadTokens":40,"reasoningTokens":10}}}},"_meta":{"eventId":"evt-grok"}}}"#;
    let fixture = fs_fixture!({
        "grok/sessions/proj/sess-grok/updates.jsonl": line,
    });
    let _env = isolated_agent_env(&fixture, "GROK_HOME", fixture.path("grok").into_os_string());
    let shared = fixture_shared("20250615", "20250615");

    let result = loader::load_rows(kind, &shared).unwrap();

    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.detected_agents, vec!["grok"]);
    assert_eq!(
        result.rows[0].metadata_agents,
        (kind != AgentReportKind::Session).then_some(vec!["grok"])
    );
    assert_eq!(result.rows[0].input_tokens, 60);
    assert_eq!(result.rows[0].cache_read_tokens, 40);
    assert_eq!(result.rows[0].output_tokens, 20);
}

#[test]
fn unified_daily_rows_use_grok_home() {
    assert_unified_rows_use_grok_home(AgentReportKind::Daily);
}

#[test]
fn unified_session_rows_use_grok_home() {
    assert_unified_rows_use_grok_home(AgentReportKind::Session);
}

fn assert_daily_family_and_session_sections_match_standalone(shared: &SharedArgs) {
    for command_kind in [AgentReportKind::Daily, AgentReportKind::Session] {
        let sections = match command_kind {
            AgentReportKind::Daily => vec![
                AgentReportKind::Daily,
                AgentReportKind::Weekly,
                AgentReportKind::Monthly,
                AgentReportKind::Session,
            ],
            AgentReportKind::Session => vec![
                AgentReportKind::Session,
                AgentReportKind::Daily,
                AgentReportKind::Weekly,
                AgentReportKind::Monthly,
            ],
            AgentReportKind::Weekly | AgentReportKind::Monthly => unreachable!(),
        };
        let section_rows = load_sections(&sections, shared).unwrap();
        let report = sections_report_json(&section_rows.sections, command_kind, false);

        for section_kind in [
            AgentReportKind::Daily,
            AgentReportKind::Weekly,
            AgentReportKind::Monthly,
            AgentReportKind::Session,
        ] {
            let standalone = load_rows(section_kind, shared).unwrap();
            let standalone_report = report_json(&standalone.rows, section_kind);
            let key = match section_kind {
                AgentReportKind::Daily => "daily",
                AgentReportKind::Weekly => "weekly",
                AgentReportKind::Monthly => "monthly",
                AgentReportKind::Session => "session",
            };
            assert_eq!(
                report.get(key).unwrap(),
                &standalone_report[key],
                "{command_kind:?} invocation should match standalone {section_kind:?}"
            );
        }
    }
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
fn uses_non_cached_codex_input_tokens_in_all_rows() {
    let mut group = CodexGroup {
        input_tokens: 100,
        cached_input_tokens: 70,
        cache_creation_tokens: 20,
        output_tokens: 5,
        total_tokens: 105,
        ..CodexGroup::default()
    };
    group.models.insert(
        "gpt-5".to_string(),
        CodexModelUsage {
            input_tokens: 100,
            cached_input_tokens: 70,
            cache_creation_tokens: 20,
            output_tokens: 5,
            total_tokens: 105,
            ..CodexModelUsage::default()
        },
    );
    let row = codex_group_row(
        "2026-01-02",
        &group,
        &PricingMap::default(),
        CodexSpeed::Standard,
    );

    assert_eq!(row.input_tokens, 10);
    assert_eq!(row.cache_creation_tokens, 20);
    assert_eq!(row.cache_read_tokens, 70);
    assert_eq!(row.total_tokens, 105);
}

#[test]
fn includes_codex_model_breakdowns_in_all_rows() {
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
    group.models.insert(
        "gpt-5-mini".to_string(),
        CodexModelUsage {
            input_tokens: 100,
            cached_input_tokens: 20,
            output_tokens: 10,
            total_tokens: 110,
            ..CodexModelUsage::default()
        },
    );
    group.models.insert(
        "gpt-5".to_string(),
        CodexModelUsage {
            input_tokens: 200,
            cached_input_tokens: 80,
            output_tokens: 40,
            total_tokens: 240,
            ..CodexModelUsage::default()
        },
    );

    let row = codex_group_row("2026-01-02", &group, &pricing, CodexSpeed::Standard);

    assert_eq!(row.model_breakdowns.len(), 2);
    assert_eq!(row.model_breakdowns[0].model_name, "gpt-5");
    assert_eq!(row.model_breakdowns[0].input_tokens, 120);
    assert_eq!(row.model_breakdowns[0].cache_read_tokens, 80);
    assert_eq!(row.model_breakdowns[0].output_tokens, 40);
    assert_eq!(row.model_breakdowns[1].model_name, "gpt-5-mini");
}

#[test]
fn aggregates_model_breakdowns_across_agents() {
    let rows = aggregate_rows(
        vec![
            AllRow {
                period: "2026-01-02".to_string(),
                agent: "codex",
                models_used: vec!["gpt-5".to_string()],
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_tokens: 0,
                cache_read_tokens: 2,
                total_tokens: 17,
                total_cost: 0.03,
                metadata: None,
                metadata_agents: Some(vec!["codex"]),
                agent_breakdowns: None,
                model_breakdowns: vec![ModelBreakdown {
                    model_name: "gpt-5".to_string(),
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 2,
                    cost: 0.03,
                    ..ModelBreakdown::default()
                }],
            },
            AllRow {
                period: "2026-01-02".to_string(),
                agent: "claude",
                models_used: vec!["gpt-5".to_string(), "claude-sonnet-4-20250514".to_string()],
                input_tokens: 30,
                output_tokens: 20,
                cache_creation_tokens: 3,
                cache_read_tokens: 4,
                total_tokens: 57,
                total_cost: 0.07,
                metadata: None,
                metadata_agents: Some(vec!["claude"]),
                agent_breakdowns: None,
                model_breakdowns: vec![
                    ModelBreakdown {
                        model_name: "gpt-5".to_string(),
                        input_tokens: 8,
                        output_tokens: 3,
                        cache_creation_tokens: 1,
                        cache_read_tokens: 2,
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
                ],
            },
        ],
        AgentReportKind::Daily,
    );

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].model_breakdowns.len(), 2);
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
}

#[test]
fn displays_total_tokens_with_cache_tokens_like_typescript_table() {
    let row = AllRow {
        period: "2026-01-02".to_string(),
        agent: "codex",
        models_used: vec!["gpt-5".to_string()],
        input_tokens: 100,
        output_tokens: 20,
        cache_creation_tokens: 0,
        cache_read_tokens: 10,
        total_tokens: 120,
        total_cost: 0.01,
        metadata: None,
        metadata_agents: Some(vec!["codex"]),
        agent_breakdowns: None,
        model_breakdowns: Vec::new(),
    };

    let cells = all_table_row(&row, false, false, false);

    assert_eq!(cells[7], "130");
}

#[test]
fn report_title_uses_detected_agents_even_when_filtered_rows_are_sparse() {
    let rows = vec![AllRow {
        period: "2026-01-02".to_string(),
        agent: "all",
        models_used: vec!["gpt-5".to_string()],
        input_tokens: 100,
        output_tokens: 20,
        cache_creation_tokens: 0,
        cache_read_tokens: 10,
        total_tokens: 120,
        total_cost: 0.01,
        metadata: None,
        metadata_agents: Some(vec!["codex"]),
        agent_breakdowns: None,
        model_breakdowns: Vec::new(),
    }];

    let title = all_report_title(
        AgentReportKind::Daily,
        &rows,
        &["amp", "claude", "codex", "opencode", "pi"],
    );

    assert_eq!(
        title,
        "Coding (Agent) CLI Usage Report - Daily\nDetected: Amp, Claude, Codex, OpenCode, pi-agent"
    );
}

#[test]
fn all_table_rows_match_main_agent_breakdown_display() {
    let row = AllRow {
        period: "2026-01-02".to_string(),
        agent: "all",
        models_used: vec!["gpt-5".to_string()],
        input_tokens: 100,
        output_tokens: 20,
        cache_creation_tokens: 0,
        cache_read_tokens: 10,
        total_tokens: 130,
        total_cost: 0.01,
        metadata: None,
        metadata_agents: Some(vec!["codex"]),
        agent_breakdowns: Some(vec![AllRow {
            period: "2026-01-02".to_string(),
            agent: "codex",
            models_used: vec!["gpt-5".to_string()],
            input_tokens: 100,
            output_tokens: 20,
            cache_creation_tokens: 0,
            cache_read_tokens: 10,
            total_tokens: 130,
            total_cost: 0.01,
            metadata: None,
            metadata_agents: Some(vec!["codex"]),
            agent_breakdowns: None,
            model_breakdowns: Vec::new(),
        }]),
        model_breakdowns: Vec::new(),
    };

    assert_eq!(
        all_table_row(&row, true, false, false),
        vec!["2026-01-02", "All", "", "100", "20", "$0.01"]
    );
    assert_eq!(
        all_table_row(
            row.agent_breakdowns.as_ref().unwrap().first().unwrap(),
            true,
            true,
            false,
        ),
        vec!["", "- Codex", "- gpt-5", "100", "20", "$0.01"]
    );
}

#[test]
fn all_report_title_lists_detected_agents() {
    let row = AllRow {
        period: "2026-01-02".to_string(),
        agent: "all",
        models_used: Vec::new(),
        input_tokens: 0,
        output_tokens: 0,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        total_tokens: 0,
        total_cost: 0.0,
        metadata: None,
        metadata_agents: Some(vec!["claude", "codex"]),
        agent_breakdowns: None,
        model_breakdowns: Vec::new(),
    };

    assert_eq!(
        all_report_title(AgentReportKind::Daily, &[row], &[]),
        "Coding (Agent) CLI Usage Report - Daily\nDetected: Claude, Codex"
    );
}

#[test]
fn compact_table_columns_omit_cache_and_total_token_metrics() {
    let (headers, aligns) = all_table_columns(AgentReportKind::Daily, true, false);

    assert_eq!(
        headers,
        vec!["Date", "Agent", "Models", "Input", "Output", "Cost (USD)"]
    );
    assert_eq!(
        aligns,
        vec![
            Align::Left,
            Align::Left,
            Align::Left,
            Align::Right,
            Align::Right,
            Align::Right,
        ]
    );
}

#[test]
fn full_table_columns_include_cache_and_total_token_metrics() {
    let (headers, aligns) = all_table_columns(AgentReportKind::Daily, false, false);

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
    assert_eq!(headers.len(), aligns.len());
}
