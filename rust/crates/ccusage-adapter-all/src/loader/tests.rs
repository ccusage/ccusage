use std::sync::Arc;

use serde_json::json;

use super::{plan::load_specs, *};
use crate::{
    adapter::codex,
    cli::{BUILT_IN_AGENT_NAMES, CodexSpeed, CostMode},
    report::sort_rows,
    sources::SOURCES,
    tests::{NO_SOURCES, aggregate_periods},
    types::{AgentRows, ReportRow},
};
use ccusage_adapter_codex::{CodexGroup, CodexModelUsage};
use ccusage_adapter_common::source::LoadRequest;
use ccusage_adapter_common::{
    filter_loaded_entries_by_date,
    rows::{SourceRow, summary_rows},
};
use ccusage_cli::NamedPiStore;
use ccusage_core::UsageSummary;
use ccusage_test_support::{Environment, fs_fixture};

fn load_pi_format_agent_rows(
    custom_path: Option<&str>,
    kind: AgentReportKind,
    shared: &SharedArgs,
    pricing: &PricingMap,
) -> Result<AgentRows> {
    pi::SOURCE.load_rows(&LoadRequest {
        kind: ccusage_adapter_common::source::LoadKind::for_report(kind),
        shared,
        options: SourceOptions {
            pi_path: custom_path,
            ..SourceOptions::default()
        },
        pricing: Some(pricing),
        progress: ProgressOwner::Caller,
    })
}

fn load_named_pi_store_rows(
    agent: &'static str,
    store_path: &str,
    kind: AgentReportKind,
    shared: &SharedArgs,
    pricing: &PricingMap,
) -> Result<AgentRows> {
    let store = pi::NamedStore::new(agent, pi::named_store_paths(store_path)?);
    store.load_rows(&LoadRequest {
        kind: ccusage_adapter_common::source::LoadKind::for_report(kind),
        shared,
        options: SourceOptions::default(),
        pricing: Some(pricing),
        progress: ProgressOwner::Caller,
    })
}

fn usage_summary(date: &str, input_tokens: u64) -> UsageSummary {
    UsageSummary {
        date: Some(date.to_string()),
        month: None,
        week: None,
        session_id: None,
        project_path: None,
        last_activity: None,
        first_activity: None,
        input_tokens,
        output_tokens: 0,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        extra_total_tokens: 0,
        total_cost: 0.0,
        credits: None,
        message_count: None,
        models_used: Vec::new(),
        model_breakdowns: Vec::new(),
        project: None,
        versions: None,
    }
}

fn finish_rows(
    kind: AgentReportKind,
    rows: Vec<SourceRow>,
    shared: &SharedArgs,
    start_of_week: WeekDay,
) -> Option<Vec<ReportRow>> {
    match Bucket::for_report(kind, start_of_week) {
        Some(bucket) => aggregate_periods(rows, bucket).map(|mut rows| {
            sort_rows(&mut rows, &shared.order.unwrap_or_default());
            rows
        }),
        None => Some(finish_sessions(rows, shared)),
    }
}

fn sorting_row(period: &str, agent: &'static str, cost: f64) -> SourceRow {
    let mut summary = usage_summary(period, 10);
    summary.total_cost = cost;
    summary_rows(agent, vec![summary], false).pop().unwrap()
}

#[test]
fn sources_start_loading_in_registry_order() {
    let shared = SharedArgs::default();
    let sources = run_sources(&shared, &NO_SOURCES).unwrap();
    let pricing = BTreeMap::new();
    let specs = load_specs(LoadKind::Day, &sources, &pricing);
    let agents = specs.iter().map(|spec| spec.agent).collect::<Vec<_>>();
    assert_eq!(agents, BUILT_IN_AGENT_NAMES);
}

#[test]
fn invalid_daily_periods_cannot_become_report_buckets() {
    for period in ["invalid", "2026-02-30", "2026-01", "2026-01-02T00:00:00Z"] {
        for bucket in [Bucket::Day, Bucket::Month, Bucket::Week(WeekDay::Monday)] {
            assert!(aggregate_periods(vec![sorting_row(period, "amp", 1.0)], bucket).is_none());
        }
    }
}

#[test]
fn pricing_maps_exclude_sources_that_load_their_own_pricing() {
    ccusage_test_support::install_pricing_snapshots();
    let shared = SharedArgs {
        offline: true,
        ..SharedArgs::default()
    };
    let sources = BTreeMap::new();
    let run_sources = run_sources(&shared, &sources).unwrap();
    let pricing_by_source = load_pricing(&run_sources);

    for agent in ["claude", "opencode"] {
        assert!(
            !pricing_by_source.contains_key(agent),
            "{agent} has its own pricing map"
        );
    }
    for agent in BUILT_IN_AGENT_NAMES {
        if !["claude", "opencode"].contains(agent) {
            assert!(
                pricing_by_source.contains_key(agent),
                "{agent} needs a pricing map"
            );
        }
    }
}

#[test]
fn display_sources_do_not_load_shared_pricing() {
    ccusage_test_support::install_pricing_snapshots();
    let shared = SharedArgs {
        mode: crate::cli::CostMode::Display,
        offline: true,
        ..SharedArgs::default()
    };
    let sources = run_sources(&shared, &NO_SOURCES).unwrap();
    assert!(load_pricing(&sources).is_empty());
}

#[test]
fn named_stores_share_the_builtin_pricing_cache() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    isolated_source_environment(&fixture).run(|| {
        let shared = SharedArgs {
            offline: true,
            pi_stores: vec![NamedPiStore {
                name: "work".to_owned(),
                path: fixture.path("work-store").to_string_lossy().into_owned(),
            }],
            ..Default::default()
        };
        let source_args = BTreeMap::new();
        let sources = run_sources(&shared, &source_args).unwrap();
        let pricing = load_pricing(&sources);
        assert_eq!(sources.len(), SOURCES.len() + 1);
        assert!(Arc::ptr_eq(&pricing["work"], &pricing["amp"]));
    })
}

#[test]
fn focused_opencode_claims_use_the_claude_source_timezone() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "claude-config/projects/project-a/native-a.jsonl": r#"{"timestamp":"2099-01-02T20:00:00.000Z","sessionId":"native-a","requestId":"req-a","costUSD":1.0,"message":{"id":"msg-a","model":"claude-opus-4-20250514","usage":{"input_tokens":100,"output_tokens":70}}}"#,
    });
    let db_path = fixture.create_dir_all("opencode").join("opencode.db");
    let db = sqlite::open(db_path).unwrap();
    db.execute("CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT, seq INTEGER, time_created INTEGER, data TEXT)").unwrap();
    let created = 4_071_067_200_000_i64;
    let data = json!({
        "time": { "created": created },
        "model": { "id": "opus", "providerID": "claude-agent" },
        "content": [{ "type": "text", "text": "hi", "native": { "sessionID": "native-a", "messageID": "m" } }],
        "tokens": { "input": 100, "output": 50 }, "cost": 0
    }).to_string();
    let mut statement = db
        .prepare("INSERT INTO session_message VALUES ('m', 'ses-agent', 'assistant', 1, ?1, ?2)")
        .unwrap();
    statement.bind((1, created)).unwrap();
    statement.bind((2, data.as_str())).unwrap();
    statement.next().unwrap();
    Environment::with_vars([
        ("HOME", Some(fixture.root().as_os_str().to_os_string())),
        (
            "CLAUDE_CONFIG_DIR",
            Some(fixture.path("claude-config").into_os_string()),
        ),
        (
            "OPENCODE_DATA_DIR",
            Some(fixture.path("opencode").into_os_string()),
        ),
    ])
    .run(|| {
        let shared = SharedArgs {
            offline: true,
            json: true,
            mode: CostMode::Display,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            since: Some("20990102".parse().unwrap()),
            until: Some("20990102".parse().unwrap()),
            ..SharedArgs::with_defaults()
        };
        let source = SourceArgs {
            shared: SharedArgs {
                timezone: Some("Asia/Tokyo".parse().unwrap()),
                ..shared.clone()
            },
            ..SourceArgs::default()
        };
        let (entries, _) =
            load_opencode_entries(&shared, AgentReportKind::Daily, false, Some(&source)).unwrap();
        assert_eq!(entries.len(), 1);
        let (entries, _) =
            load_opencode_entries(&shared, AgentReportKind::Daily, false, None).unwrap();
        assert!(entries.is_empty());
    })
}

#[test]
fn source_claude_project_filters_daily_and_session_rows_without_filtering_other_sources() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "claude-config/projects/project-a/session-a.jsonl": r#"{"timestamp":"2099-01-02T00:00:00.000Z","sessionId":"session-a","costUSD":0.01,"message":{"usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":0,"cache_read_input_tokens":0},"model":"claude-sonnet-4-20250514","id":"msg-a"}}"#,
        "claude-config/projects/project-b/session-b.jsonl": r#"{"timestamp":"2099-01-02T00:00:00.000Z","sessionId":"session-b","costUSD":0.02,"message":{"usage":{"input_tokens":1000,"output_tokens":500,"cache_creation_input_tokens":0,"cache_read_input_tokens":0},"model":"claude-sonnet-4-20250514","id":"msg-b"}}"#,
        "pi/sessions/project-pi/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":7,"output":3}}}"#,
    });
    isolated_source_environment(&fixture).run(|| {
        let shared = SharedArgs {
            json: true,
            mode: CostMode::Display,
            offline: true,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };
        let sources = BTreeMap::from([(
            "claude".to_string(),
            SourceArgs {
                options: ccusage_core::cli::AgentOptions {
                    claude_project: Some("project-a".to_string()),
                    ..Default::default()
                },
                shared: shared.clone(),
            },
        )]);

        let daily = load_rows(AgentReportKind::Daily, &shared, &sources, WeekDay::Monday).unwrap();

        assert_eq!(daily.rows.len(), 1);
        assert_eq!(daily.rows[0].period, "2099-01-02");
        assert_eq!(
            daily.rows[0].agents().map(<[_]>::to_vec),
            Some(vec!["claude", "pi"])
        );
        assert_eq!(daily.rows[0].input_tokens, 17);
        assert_eq!(daily.rows[0].output_tokens, 8);
        assert!((daily.rows[0].total_cost - 0.01).abs() < f64::EPSILON);

        let sessions =
            load_rows(AgentReportKind::Session, &shared, &sources, WeekDay::Monday).unwrap();

        assert!(
            sessions
                .rows
                .iter()
                .any(|row| row.agent() == "claude" && row.period == "session-a")
        );
        assert!(
            !sessions
                .rows
                .iter()
                .any(|row| row.agent() == "claude" && row.period == "session-b")
        );
        assert!(
            sessions
                .rows
                .iter()
                .any(|row| row.agent() == "pi" && row.period == "pi-session")
        );
    })
}

fn isolated_source_environment(fixture: &ccusage_test_support::Fixture) -> Environment {
    isolated_source_environment_with_pi_agent_dir(fixture, "pi/sessions")
}

fn isolated_source_environment_with_pi_agent_dir(
    fixture: &ccusage_test_support::Fixture,
    pi_agent_dir: &str,
) -> Environment {
    fixture.create_dir_all("claude-config/projects");
    fixture.create_dir_all("pi/sessions");
    fixture.create_dir_all("empty-xdg-config");
    fixture.create_dir_all("empty-xdg-data");
    Environment::with_vars([
        ("HOME", Some(fixture.root().as_os_str().to_os_string())),
        (
            "USERPROFILE",
            Some(fixture.path("empty-userprofile").into_os_string()),
        ),
        (
            "CLAUDE_CONFIG_DIR",
            Some(fixture.path("claude-config").into_os_string()),
        ),
        (
            "CODEX_HOME",
            Some(fixture.path("codex-home").into_os_string()),
        ),
        ("OPENCODE_DATA_DIR", None),
        ("AMP_DATA_DIR", None),
        ("DROID_SESSIONS_DIR", None),
        ("CODEBUFF_DATA_DIR", None),
        ("HERMES_HOME", None),
        (
            "PI_AGENT_DIR",
            Some(fixture.path(pi_agent_dir).into_os_string()),
        ),
        ("GOOSE_PATH_ROOT", None),
        (
            "OPENCLAW_DIR",
            Some(fixture.path("openclaw-home").into_os_string()),
        ),
        ("KILO_DATA_DIR", None),
        ("COPILOT_OTEL_FILE_EXPORTER_PATH", None),
        ("COPILOT_HOME", None),
        ("GEMINI_DATA_DIR", None),
        ("KIMI_DATA_DIR", None),
        ("QWEN_DATA_DIR", None),
        ("GROK_HOME", None),
        ("ZCODE_HOME", None),
        ("ANTIGRAVITY_DATA_DIR", None),
        ("CLINE_DATA_DIR", None),
        ("DSH_HOME", None),
        ("APPDATA", None),
        (
            "XDG_CONFIG_HOME",
            Some(fixture.path("empty-xdg-config").into_os_string()),
        ),
        (
            "XDG_DATA_HOME",
            Some(fixture.path("empty-xdg-data").into_os_string()),
        ),
    ])
}

fn codex_rollout(date: &str, model: &str, input_tokens: u64, output_tokens: u64) -> String {
    [
        serde_json::json!({"type":"turn_context","payload":{"model":model}}).to_string(),
        serde_json::json!({
            "timestamp": format!("{date}T00:00:00.000Z"),
            "type": "event_msg",
            "payload": {
                "type": "thread_settings_applied",
                "thread_settings": {"service_tier": "default"},
            },
        })
        .to_string(),
        serde_json::json!({
            "timestamp": format!("{date}T00:00:01.000Z"),
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "model": model,
                    "last_token_usage": {
                        "input_tokens": input_tokens,
                        "output_tokens": output_tokens,
                        "total_tokens": input_tokens + output_tokens,
                    },
                    "total_token_usage": {
                        "input_tokens": input_tokens,
                        "output_tokens": output_tokens,
                        "total_tokens": input_tokens + output_tokens,
                    },
                },
            },
        })
        .to_string(),
    ]
    .join("\n")
}

#[test]
fn session_cost_order_honors_explicit_ascending_order() {
    let shared = SharedArgs {
        order: Some(crate::cli::SortOrder::Asc),
        ..SharedArgs::default()
    };
    let rows = vec![
        sorting_row("a-expensive", "claude", 20.0),
        sorting_row("z-cheap", "codex", 1.0),
        sorting_row("z-cheap", "claude", 1.0),
        sorting_row("b-zero", "claude", 0.0),
    ];
    let sorted = finish_rows(AgentReportKind::Session, rows, &shared, WeekDay::Monday).unwrap();
    let keys: Vec<_> = sorted
        .iter()
        .map(|row| (row.period.as_str(), row.agent()))
        .collect();
    assert_eq!(
        keys,
        [
            ("b-zero", "claude"),
            ("z-cheap", "claude"),
            ("z-cheap", "codex"),
            ("a-expensive", "claude")
        ]
    );
}

#[test]
fn session_cost_order_uses_id_then_agent_for_ties() {
    for (order, order_explicit) in [
        (crate::cli::SortOrder::Asc, false),
        (crate::cli::SortOrder::Desc, true),
    ] {
        let rows = vec![
            sorting_row("b-tie", "codex", 20.0),
            sorting_row("a-zero", "claude", 0.0),
            sorting_row("z-tie", "claude", 20.0),
            sorting_row("b-tie", "claude", 20.0),
            sorting_row("c-cheap", "codex", 1.0),
        ];
        let shared = SharedArgs {
            order: order_explicit.then_some(order),
            ..SharedArgs::default()
        };
        let sorted = finish_rows(AgentReportKind::Session, rows, &shared, WeekDay::Monday).unwrap();
        let keys: Vec<_> = sorted
            .iter()
            .map(|row| (row.period.as_str(), row.agent()))
            .collect();
        assert_eq!(
            keys,
            [
                ("b-tie", "claude"),
                ("b-tie", "codex"),
                ("z-tie", "claude"),
                ("c-cheap", "codex"),
                ("a-zero", "claude"),
            ]
        );
    }
}

#[test]
fn session_cost_order_handles_empty_and_single_row_reports() {
    let shared = SharedArgs::default();
    assert!(
        finish_rows(
            AgentReportKind::Session,
            Vec::new(),
            &shared,
            WeekDay::Monday
        )
        .unwrap()
        .is_empty()
    );
    let rows = finish_rows(
        AgentReportKind::Session,
        vec![sorting_row("only-session", "claude", 0.0)],
        &shared,
        WeekDay::Monday,
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].period, "only-session");
}

#[test]
fn period_reports_keep_chronological_order_regardless_of_cost() {
    for kind in [
        AgentReportKind::Daily,
        AgentReportKind::Weekly,
        AgentReportKind::Monthly,
    ] {
        for (order, expected_costs) in [
            (crate::cli::SortOrder::Asc, [1.0, 20.0, 3.0]),
            (crate::cli::SortOrder::Desc, [3.0, 20.0, 1.0]),
        ] {
            let rows = vec![
                sorting_row("2026-03-02", "claude", 3.0),
                sorting_row("2026-01-05", "claude", 1.0),
                sorting_row("2026-02-02", "claude", 20.0),
            ];
            let shared = SharedArgs {
                order: Some(order),
                ..SharedArgs::default()
            };
            let sorted = finish_rows(kind, rows, &shared, WeekDay::Monday).unwrap();
            let costs: Vec<_> = sorted.iter().map(|row| row.total_cost).collect();
            assert_eq!(costs, expected_costs, "{kind:?} {order:?}");
        }
    }
}

#[test]
fn unified_row_matches_focused_cost_for_mixed_recorded_tiers() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "gpt-test": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000002,
                    "provider_specific_entry": { "fast": 2 }
                }
            }"#,
    );
    let mut usage = CodexModelUsage::from_buckets(
        ccusage_adapter_codex::CodexUsageBucket {
            input_tokens: 30,
            ..Default::default()
        },
        {
            let mut tiers = ccusage_adapter_codex::TierBuckets::default();
            tiers[ccusage_adapter_codex::CodexServiceTier::Standard] = codex::CodexUsageBucket {
                input_tokens: 10,
                ..codex::CodexUsageBucket::default()
            };
            tiers[ccusage_adapter_codex::CodexServiceTier::Fast] = codex::CodexUsageBucket {
                input_tokens: 10,
                ..codex::CodexUsageBucket::default()
            };
            tiers
        },
    );
    usage.total_tokens = 30;
    let mut group = CodexGroup {
        input_tokens: 30,
        total_tokens: 30,
        ..CodexGroup::default()
    };
    group.models.insert("gpt-test".to_string(), usage);
    let speed = codex::CodexSpeedPolicy::Auto;

    let focused_cost = codex::calculate_group_cost(&group, Some(&pricing), speed);
    let unified = codex::unified_row_from_group(
        ccusage_adapter_common::source::LoadKind::Session,
        "2026-07-22",
        &group,
        Some(&pricing),
        speed,
        CostMode::Calculate,
    );

    assert!((focused_cost - 40e-6).abs() < f64::EPSILON);
    assert!((unified.total_cost - focused_cost).abs() < f64::EPSILON);
    assert!((unified.model_breakdowns[0].cost - focused_cost).abs() < f64::EPSILON);
}

#[test]
fn unified_row_reports_codex_cache_write_tokens_and_cost() {
    ccusage_test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    let mut usage = CodexModelUsage::from_buckets(
        ccusage_adapter_codex::CodexUsageBucket {
            input_tokens: 935_040,
            cached_input_tokens: 875_306,
            cache_creation_tokens: 57_610,
            output_tokens: 11_150,
            long_context_input_tokens: 935_040,
            long_context_cached_input_tokens: 875_306,
            long_context_cache_creation_tokens: 57_610,
            long_context_output_tokens: 11_150,
        },
        Default::default(),
    );
    usage.total_tokens = 946_190;
    let mut group = CodexGroup {
        input_tokens: 935_040,
        cached_input_tokens: 875_306,
        cache_creation_tokens: 57_610,
        output_tokens: 11_150,
        total_tokens: 946_190,
        ..CodexGroup::default()
    };
    group.models.insert("gpt-5.6-terra".to_string(), usage);

    let row = codex::unified_row_from_group(
        ccusage_adapter_common::source::LoadKind::Session,
        "2026-08-20",
        &group,
        Some(&pricing),
        codex::CodexSpeedPolicy::Forced(codex::CodexServiceTier::Standard),
        CostMode::Calculate,
    );
    let expected_cost = 2_124.0 * 4e-6 + 875_306.0 * 0.4e-6 + 57_610.0 * 5e-6 + 11_150.0 * 18e-6;

    assert_eq!(row.input_tokens, 2_124);
    assert_eq!(row.cache_creation_tokens, 57_610);
    assert_eq!(row.cache_read_tokens, 875_306);
    assert!((row.total_cost - expected_cost).abs() < 1e-12);
    assert_eq!(row.model_breakdowns[0].input_tokens, 2_124);
    assert_eq!(row.model_breakdowns[0].cache_creation_tokens, 57_610);
    assert_eq!(row.model_breakdowns[0].cache_read_tokens, 875_306);
    assert!((row.model_breakdowns[0].cost - expected_cost).abs() < 1e-12);
}

fn pi_path_subcommand_rows(
    store_path: &str,
    kind: AgentReportKind,
    shared: &SharedArgs,
    pricing: &PricingMap,
) -> Result<Vec<SourceRow>> {
    let (mut entries, _) = ccusage_adapter_common::testing::load_entries(
        &pi::SOURCE,
        shared,
        SourceOptions {
            pi_path: Some(store_path),
            ..SourceOptions::default()
        },
        Some(pricing),
    )?;
    filter_loaded_entries_by_date(&mut entries, shared);
    let summaries = ccusage_adapter_common::source::EntrySource::summarize(
        &pi::SOURCE,
        &entries,
        ccusage_adapter_common::source::SummaryKind::for_report(
            kind,
            ccusage_core::cli::WeekDay::Sunday,
        ),
    )?;
    Ok(summary_rows("pi", summaries, true).into_iter().collect())
}

fn usage_fingerprint(
    rows: &[impl std::ops::Deref<Target = ccusage_adapter_common::rows::RowUsage>],
) -> Vec<(String, u64, u64, u64, u64, u64)> {
    rows.iter()
        .map(|row| {
            (
                row.period.clone(),
                row.input_tokens,
                row.output_tokens,
                row.cache_creation_tokens,
                row.cache_read_tokens,
                row.total_tokens,
            )
        })
        .collect()
}

#[test]
fn load_rows_filters_named_pi_store_sessions_like_pi_path_subcommand() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "omp/sessions/project-a/agent_inside-session.jsonl": r#"{"type":"message","timestamp":"2026-07-04T18:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":1}}}"#,
        "omp/sessions/project-a/agent_outside-session.jsonl": r#"{"type":"message","timestamp":"2026-07-05T18:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":20,"output":2}}}"#,
    });
    isolated_source_environment_with_pi_agent_dir(&fixture, "empty-default").run(|| {
        let store_path = fixture.path("omp/sessions").to_string_lossy().into_owned();
        let shared = SharedArgs {
            json: true,
            mode: crate::cli::CostMode::Display,
            offline: true,
            timezone: Some("America/Boise".parse().unwrap()),
            since: Some("20260704".parse().unwrap()),
            until: Some("20260704".parse().unwrap()),
            pi_stores: vec![NamedPiStore {
                name: "omp".to_string(),
                path: store_path.clone(),
            }],
            ..SharedArgs::default()
        };

        let result = load_rows(
            AgentReportKind::Session,
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        )
        .unwrap();
        let omp_rows = result
            .rows
            .iter()
            .filter(|row| row.agent() == "omp")
            .collect::<Vec<_>>();
        let pi_rows = pi_path_subcommand_rows(
            &store_path,
            AgentReportKind::Session,
            &shared,
            &PricingMap::default(),
        )
        .unwrap();

        assert_eq!(omp_rows.len(), 1);
        assert_eq!(omp_rows[0].period, "inside-session");
        assert_eq!(omp_rows[0].input_tokens, 10);
        assert_eq!(omp_rows[0].output_tokens, 1);
        assert_eq!(
            usage_fingerprint(&pi_rows),
            vec![("inside-session".to_string(), 10, 1, 0, 0, 11)]
        );
    })
}

#[test]
fn named_pi_store_returns_sanitized_diagnostics_with_healthy_usage() {
    let fixture = fs_fixture!({
        "store/project-a/agent_session-a.jsonl": [
            r#"{"type":"session","id":"session-a","timestamp":"2026-01-01T00:00:00.000Z"}"#,
            r#"{"type":"message","message":{"role":"assistant","usage":"SECRET_CANARY"}}"#,
            r#"{"type":"message","timestamp":"2026-01-02T10:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":1}}}"#,
        ]
        .join("\n"),
    });
    let shared = SharedArgs {
        mode: CostMode::Display,
        debug: true,
        offline: true,
        ..SharedArgs::default()
    };

    let rows = load_named_pi_store_rows(
        "omp",
        fixture.path("store").to_str().unwrap(),
        AgentReportKind::Daily,
        &shared,
        &PricingMap::default(),
    )
    .unwrap();

    assert!(rows.detected);
    assert_eq!(rows.rows.len(), 1);
    assert!(rows.diagnostics.warnings(&shared).iter().any(|warning| {
        warning.contains("invalid data")
            && warning.starts_with("omp:")
            && warning.contains("agent_session-a.jsonl:2")
            && !warning.contains("SECRET_CANARY")
    }));
}

#[test]
fn named_pi_store_windowed_period_rows_match_pi_path_subcommand() {
    let fixture = fs_fixture!({
        "omp/sessions/project-a/agent_july-third.jsonl": r#"{"type":"message","timestamp":"2026-07-03T18:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":1}}}"#,
        "omp/sessions/project-a/agent_july-fourth.jsonl": r#"{"type":"message","timestamp":"2026-07-04T18:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":20,"output":2}}}"#,
        "omp/sessions/project-a/agent_july-fifth.jsonl": r#"{"type":"message","timestamp":"2026-07-05T18:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":30,"output":3}}}"#,
    });
    let shared = SharedArgs {
        mode: crate::cli::CostMode::Display,
        timezone: Some("America/Boise".parse().unwrap()),
        since: Some("20260703".parse().unwrap()),
        until: Some("20260704".parse().unwrap()),
        ..SharedArgs::default()
    };
    let store_path = fixture.path("omp/sessions").to_string_lossy().into_owned();
    let pricing = PricingMap::default();
    let cases = [
        AgentReportKind::Daily,
        AgentReportKind::Weekly,
        AgentReportKind::Monthly,
    ];

    for kind in cases {
        let named_rows = load_named_pi_store_rows("omp", &store_path, kind, &shared, &pricing)
            .unwrap()
            .rows;
        let named_rows = finish_rows(
            kind,
            named_rows,
            &shared,
            ccusage_core::cli::WeekDay::Sunday,
        )
        .unwrap();
        let pi_rows = pi_path_subcommand_rows(&store_path, kind, &shared, &pricing).unwrap();

        assert_eq!(usage_fingerprint(&named_rows), usage_fingerprint(&pi_rows));
    }
}

#[test]
fn default_pi_unified_session_window_keeps_until_day_like_pi_path() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "pi/sessions/project-a/agent_until-day.jsonl": r#"{"type":"message","timestamp":"2026-07-04T18:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":1}}}"#,
        "pi/sessions/project-a/agent_after-day.jsonl": r#"{"type":"message","timestamp":"2026-07-05T18:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":20,"output":2}}}"#,
    });
    isolated_source_environment_with_pi_agent_dir(&fixture, "pi/sessions").run(|| {
        let shared = SharedArgs {
            json: true,
            mode: crate::cli::CostMode::Display,
            offline: true,
            timezone: Some("America/Boise".parse().unwrap()),
            since: Some("20260704".parse().unwrap()),
            until: Some("20260704".parse().unwrap()),
            ..SharedArgs::default()
        };

        let result = load_rows(
            AgentReportKind::Session,
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        )
        .unwrap();
        let pi_rows = result
            .rows
            .iter()
            .filter(|row| row.agent() == "pi")
            .cloned()
            .collect::<Vec<_>>();
        let pi_path_rows = pi_path_subcommand_rows(
            fixture.path("pi/sessions").to_str().unwrap(),
            AgentReportKind::Session,
            &shared,
            &PricingMap::default(),
        )
        .unwrap();

        assert_eq!(
            usage_fingerprint(&pi_rows),
            usage_fingerprint(&pi_path_rows)
        );
        assert_eq!(
            usage_fingerprint(&pi_rows),
            vec![("until-day".to_string(), 10, 1, 0, 0, 11)]
        );
    })
}

#[test]
fn unified_claude_session_window_keeps_sessions_last_active_on_the_until_day() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "claude-config/projects/project-a/until-day.jsonl": r#"{"timestamp":"2026-07-05T03:00:00.000Z","sessionId":"until-day","costUSD":0.01,"message":{"usage":{"input_tokens":10,"output_tokens":1},"model":"claude-sonnet-4-20250514","id":"msg-until"}}"#,
        "claude-config/projects/project-a/after-day.jsonl": r#"{"timestamp":"2026-07-06T03:00:00.000Z","sessionId":"after-day","costUSD":0.02,"message":{"usage":{"input_tokens":20,"output_tokens":2},"model":"claude-sonnet-4-20250514","id":"msg-after"}}"#,
    });
    isolated_source_environment(&fixture).run(|| {
        let shared = SharedArgs {
            json: true,
            mode: CostMode::Display,
            offline: true,
            timezone: Some("America/Boise".parse().unwrap()),
            since: Some("20260704".parse().unwrap()),
            until: Some("20260704".parse().unwrap()),
            ..SharedArgs::default()
        };

        let result = load_rows(
            AgentReportKind::Session,
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        )
        .unwrap();
        let claude_rows = result
            .rows
            .iter()
            .filter(|row| row.agent() == "claude")
            .cloned()
            .collect::<Vec<_>>();

        assert_eq!(
            usage_fingerprint(&claude_rows),
            vec![("until-day".to_string(), 10, 1, 0, 0, 11)]
        );
    })
}

#[test]
fn default_pi_session_rows_derive_project_from_the_store_root() {
    let fixture = fs_fixture!({
        "default-root/archive/sessions/project-a/agent_default-session.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":100,"output":200}}}"#,
    });
    let shared = SharedArgs {
        mode: crate::cli::CostMode::Display,
        ..SharedArgs::default()
    };

    let rows = load_pi_format_agent_rows(
        Some(fixture.path("default-root").to_str().unwrap()),
        AgentReportKind::Session,
        &shared,
        &PricingMap::default(),
    )
    .unwrap();

    assert_eq!(rows.rows.len(), 1);
    assert_eq!(rows.rows[0].agent, "pi");
    assert_eq!(
        rows.rows[0]
            .metadata
            .as_ref()
            .unwrap()
            .project_path
            .as_ref()
            .unwrap(),
        "archive"
    );
}

#[test]
fn named_pi_store_rows_keep_agent_model_and_project_metadata() {
    let fixture = fs_fixture!({
        "omp/sessions/project-a/agent_omp-session.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":100,"output":200}}}"#,
    });
    let shared = SharedArgs {
        mode: crate::cli::CostMode::Display,
        ..SharedArgs::default()
    };

    let rows = load_named_pi_store_rows(
        "omp",
        fixture.path("omp/sessions").to_str().unwrap(),
        AgentReportKind::Session,
        &shared,
        &PricingMap::default(),
    )
    .unwrap();

    assert!(rows.detected);
    assert_eq!(rows.rows.len(), 1);
    assert_eq!(rows.rows[0].agent, "omp");
    assert_eq!(rows.rows[0].models_used, vec!["[omp] gpt-5"]);
    assert_eq!(
        rows.rows[0]
            .metadata
            .as_ref()
            .unwrap()
            .project_path
            .as_ref()
            .unwrap(),
        "project-a"
    );
}

#[test]
fn named_pi_store_sessions_split_by_project_like_the_default_store() {
    let session = |input: u64| {
        [
            r#"{"type":"session","id":"same-session","timestamp":"2026-01-01T00:00:00.000Z"}"#.to_string(),
            format!(
                r#"{{"type":"message","timestamp":"2026-01-02T10:00:00.000Z","message":{{"role":"assistant","model":"gpt-5","usage":{{"input":{input},"output":1}}}}}}"#
            ),
        ]
        .join("\n")
    };
    let fixture = fs_fixture!({
        "omp/sessions/project-a/agent_same-session.jsonl": session(10),
        "omp/sessions/project-b/agent_same-session.jsonl": session(20),
    });
    let shared = SharedArgs {
        mode: crate::cli::CostMode::Display,
        ..SharedArgs::default()
    };
    let store_path = fixture.path("omp/sessions");

    let named = load_named_pi_store_rows(
        "omp",
        store_path.to_str().unwrap(),
        AgentReportKind::Session,
        &shared,
        &PricingMap::default(),
    )
    .unwrap();
    let default = load_pi_format_agent_rows(
        store_path.to_str(),
        AgentReportKind::Session,
        &shared,
        &PricingMap::default(),
    )
    .unwrap();

    let sessions = |rows: &AgentRows| {
        rows.rows
            .iter()
            .map(|row| {
                (
                    row.metadata
                        .as_ref()
                        .unwrap()
                        .project_path
                        .as_ref()
                        .unwrap()
                        .clone(),
                    row.input_tokens,
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        sessions(&named),
        [("project-a".to_string(), 10), ("project-b".to_string(), 20)]
    );
    assert_eq!(sessions(&named), sessions(&default));
}

#[test]
fn named_pi_store_rows_derive_project_relative_to_store_root() {
    let fixture = fs_fixture!({
        "store/project-b/agent_omp-session.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":100,"output":200}}}"#,
        "backups/sessions/omp-copy/project-c/agent_omp-session.jsonl": r#"{"type":"message","timestamp":"2026-01-03T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":100,"output":200}}}"#,
    });
    let shared = SharedArgs {
        mode: crate::cli::CostMode::Display,
        ..SharedArgs::default()
    };

    let plain_root = load_named_pi_store_rows(
        "omp",
        fixture.path("store").to_str().unwrap(),
        AgentReportKind::Session,
        &shared,
        &PricingMap::default(),
    )
    .unwrap();
    let earlier_sessions_root = load_named_pi_store_rows(
        "omp",
        fixture.path("backups/sessions/omp-copy").to_str().unwrap(),
        AgentReportKind::Session,
        &shared,
        &PricingMap::default(),
    )
    .unwrap();

    assert_eq!(
        plain_root.rows[0]
            .metadata
            .as_ref()
            .unwrap()
            .project_path
            .as_ref()
            .unwrap(),
        "project-b"
    );
    assert_eq!(
        earlier_sessions_root.rows[0]
            .metadata
            .as_ref()
            .unwrap()
            .project_path
            .as_ref()
            .unwrap(),
        "project-c"
    );
}

#[test]
fn named_pi_store_absent_path_is_clean_empty() {
    let fixture = fs_fixture!({});
    let shared = SharedArgs {
        mode: crate::cli::CostMode::Display,
        ..SharedArgs::default()
    };

    let rows = load_named_pi_store_rows(
        "omp",
        fixture.path("missing/sessions").to_str().unwrap(),
        AgentReportKind::Daily,
        &shared,
        &PricingMap::default(),
    )
    .unwrap();

    assert!(!rows.detected);
    assert!(rows.rows.is_empty());
}

#[test]
fn load_rows_rejects_named_pi_store_that_collides_with_default_pi_path() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        ".pi/agent/sessions/project-a/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":20}}}"#,
    });
    isolated_source_environment_with_pi_agent_dir(&fixture, ".pi/agent/sessions").run(|| {
        let shared = SharedArgs {
            json: true,
            mode: crate::cli::CostMode::Display,
            offline: true,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            since: Some("20990102".parse().unwrap()),
            until: Some("20990102".parse().unwrap()),
            pi_stores: vec![NamedPiStore {
                name: "omp".to_string(),
                path: fixture
                    .path(".pi/agent/sessions")
                    .to_string_lossy()
                    .into_owned(),
            }],
            ..SharedArgs::default()
        };

        let Err(error) = load_rows(
            AgentReportKind::Daily,
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        ) else {
            panic!("expected default pi path collision");
        };

        assert!(error.to_string().contains("Invalid ccusage config"));
        assert!(error.to_string().contains("omp"));
        assert!(error.to_string().contains("default pi"));
    })
}

#[test]
fn load_rows_rejects_named_pi_store_that_collides_with_another_store() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "shared/sessions/project-a/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":20}}}"#,
    });
    isolated_source_environment_with_pi_agent_dir(&fixture, "empty-default").run(|| {
        let path = fixture
            .path("shared/sessions")
            .to_string_lossy()
            .into_owned();
        let shared = SharedArgs {
            json: true,
            mode: crate::cli::CostMode::Display,
            offline: true,
            since: Some("20990102".parse().unwrap()),
            until: Some("20990102".parse().unwrap()),
            pi_stores: vec![
                NamedPiStore {
                    name: "omp".to_string(),
                    path: path.clone(),
                },
                NamedPiStore {
                    name: "fork".to_string(),
                    path,
                },
            ],
            ..SharedArgs::default()
        };

        let Err(error) = load_rows(
            AgentReportKind::Daily,
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        ) else {
            panic!("expected named pi store path collision");
        };

        assert!(error.to_string().contains("Invalid ccusage config"));
        assert!(error.to_string().contains("fork"));
        assert!(error.to_string().contains("omp"));
    })
}

#[test]
fn load_rows_rejects_named_pi_store_nested_inside_default_pi_path() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        ".pi/agent/sessions/project-a/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":20}}}"#,
    });
    isolated_source_environment_with_pi_agent_dir(&fixture, ".pi/agent/sessions").run(|| {
        let shared = SharedArgs {
            json: true,
            mode: crate::cli::CostMode::Display,
            offline: true,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            since: Some("20990102".parse().unwrap()),
            until: Some("20990102".parse().unwrap()),
            pi_stores: vec![NamedPiStore {
                name: "omp".to_string(),
                path: fixture
                    .path(".pi/agent/sessions/project-a")
                    .to_string_lossy()
                    .into_owned(),
            }],
            ..SharedArgs::default()
        };

        let Err(error) = load_rows(
            AgentReportKind::Daily,
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        ) else {
            panic!("expected nested default pi path collision");
        };

        assert!(error.to_string().contains("Invalid ccusage config"));
        assert!(error.to_string().contains("omp"));
        assert!(error.to_string().contains("default pi"));
    })
}

#[test]
fn load_rows_rejects_named_pi_store_partially_overlapping_another_store() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "shared/sessions/project-a/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":20}}}"#,
    });
    isolated_source_environment_with_pi_agent_dir(&fixture, "empty-default").run(|| {
        let shared_path = fixture
            .path("shared/sessions")
            .to_string_lossy()
            .into_owned();
        let distinct = fixture.create_dir_all("fork-only/sessions");
        let shared = SharedArgs {
            json: true,
            mode: crate::cli::CostMode::Display,
            offline: true,
            since: Some("20990102".parse().unwrap()),
            until: Some("20990102".parse().unwrap()),
            pi_stores: vec![
                NamedPiStore {
                    name: "omp".to_string(),
                    path: shared_path.clone(),
                },
                NamedPiStore {
                    name: "fork".to_string(),
                    path: format!("{}, {}", distinct.display(), shared_path),
                },
            ],
            ..SharedArgs::default()
        };

        let Err(error) = load_rows(
            AgentReportKind::Daily,
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        ) else {
            panic!("expected partial named pi store path collision");
        };

        assert!(error.to_string().contains("Invalid ccusage config"));
        assert!(error.to_string().contains("fork"));
        assert!(error.to_string().contains("omp"));
    })
}

#[test]
fn load_rows_includes_named_pi_store_through_production_wiring() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "omp/sessions/project-a/agent_omp-session.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":30,"output":40}}}"#,
    });
    isolated_source_environment_with_pi_agent_dir(&fixture, "empty-default").run(|| {
        let shared = SharedArgs {
            json: true,
            mode: crate::cli::CostMode::Display,
            offline: true,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            since: Some("20990102".parse().unwrap()),
            until: Some("20990102".parse().unwrap()),
            pi_stores: vec![NamedPiStore {
                name: "omp".to_string(),
                path: fixture.path("omp/sessions").to_string_lossy().into_owned(),
            }],
            ..SharedArgs::default()
        };

        let result = load_rows(
            AgentReportKind::Daily,
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        )
        .unwrap();

        assert!(result.detected_agents.contains(&"omp"));
        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].agents().map(<[_]>::to_vec),
            Some(vec!["omp"])
        );
        assert_eq!(result.rows[0].models_used, vec!["[omp] gpt-5"]);
        assert_eq!(result.rows[0].input_tokens, 30);
        assert_eq!(result.rows[0].output_tokens, 40);
    })
}

#[test]
fn load_rows_reads_the_default_pi_store_from_its_source_pi_path() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "custom/pi/project-a/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":30,"output":40}}}"#,
    });
    isolated_source_environment(&fixture).run(|| {
        let shared = SharedArgs {
            json: true,
            mode: crate::cli::CostMode::Display,
            offline: true,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            since: Some("20990102".parse().unwrap()),
            until: Some("20990102".parse().unwrap()),
            ..SharedArgs::default()
        };
        let sources = BTreeMap::from([(
            "pi".to_string(),
            SourceArgs {
                options: ccusage_core::cli::AgentOptions {
                    pi_path: Some(fixture.path("custom/pi").to_string_lossy().into_owned()),
                    ..Default::default()
                },
                shared: shared.clone(),
            },
        )]);

        let unified = load_rows(
            AgentReportKind::Daily,
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        )
        .unwrap();
        let result = load_rows(AgentReportKind::Daily, &shared, &sources, WeekDay::Monday).unwrap();

        assert!(!unified.detected_agents.contains(&"pi"));
        assert!(result.detected_agents.contains(&"pi"));
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].agents().map(<[_]>::to_vec), Some(vec!["pi"]));
        assert_eq!(result.rows[0].input_tokens, 30);
        assert_eq!(result.rows[0].output_tokens, 40);
    })
}

#[test]
fn load_rows_filters_a_source_by_its_own_date_window() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "pi/sessions/project-a/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":30,"output":40}}}"#,
    });
    isolated_source_environment(&fixture).run(|| {
        let shared = SharedArgs {
            json: true,
            mode: crate::cli::CostMode::Display,
            offline: true,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            since: Some("20990101".parse().unwrap()),
            until: Some("20990103".parse().unwrap()),
            ..SharedArgs::default()
        };
        let sources = BTreeMap::from([(
            "pi".to_string(),
            SourceArgs {
                shared: SharedArgs {
                    since: Some("20990105".parse().unwrap()),
                    until: Some("20990106".parse().unwrap()),
                    ..shared.clone()
                },
                ..SourceArgs::default()
            },
        )]);

        let unified = load_rows(
            AgentReportKind::Daily,
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        )
        .unwrap();
        let result = load_rows(AgentReportKind::Daily, &shared, &sources, WeekDay::Monday).unwrap();

        assert_eq!(unified.rows.len(), 1);
        assert!(result.rows.is_empty());
        assert!(result.detected_agents.contains(&"pi"));
    })
}

#[test]
fn loads_codex_and_pi_with_source_pricing_speed_and_date_options() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "codex-home/sessions/2026/07/21/before.jsonl": codex_rollout(
            "2026-07-21",
            "gpt-source-config",
            900,
            100,
        ),
        "codex-home/sessions/2026/07/22/in-range.jsonl": codex_rollout(
            "2026-07-22",
            "gpt-source-config",
            1000,
            500,
        ),
        "pi/sessions/project-a/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2026-07-22T00:00:00.000Z","message":{"role":"assistant","model":"gpt-source-config","usage":{"input":1000,"output":500}}}"#,
    });
    isolated_source_environment(&fixture).run(|| {
        let shared = SharedArgs {
            mode: CostMode::Calculate,
            offline: true,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            since: Some("20260720".parse().unwrap()),
            until: Some("20260723".parse().unwrap()),
            ..SharedArgs::default()
        };
        let pricing_overrides = BTreeMap::from([(
            "gpt-source-config".to_string(),
            crate::cli::PricingOverride {
                input_cost_per_token: Some(1e-6),
                output_cost_per_token: Some(2e-6),
                fast_multiplier: Some(3.0),
                ..Default::default()
            },
        )]);
        let pi_pricing_overrides = BTreeMap::from([(
            "gpt-source-config".to_string(),
            crate::cli::PricingOverride {
                input_cost_per_token: Some(5e-6),
                output_cost_per_token: Some(10e-6),
                ..Default::default()
            },
        )]);
        let sources = BTreeMap::from([
            (
                "codex".to_string(),
                SourceArgs {
                    options: ccusage_core::cli::AgentOptions {
                        codex_speed: CodexSpeed::Fast,
                        ..Default::default()
                    },
                    shared: SharedArgs {
                        since: Some("20260722".parse().unwrap()),
                        until: Some("20260722".parse().unwrap()),
                        pricing_overrides,
                        ..shared.clone()
                    },
                },
            ),
            (
                "pi".to_string(),
                SourceArgs {
                    shared: SharedArgs {
                        pricing_overrides: pi_pricing_overrides,
                        ..shared.clone()
                    },
                    ..SourceArgs::default()
                },
            ),
        ]);

        let result = load_rows(AgentReportKind::Daily, &shared, &sources, WeekDay::Monday).unwrap();

        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].agents().map(<[_]>::to_vec),
            Some(vec!["codex", "pi"])
        );
        let breakdowns = result.rows[0].agent_breakdowns().unwrap();
        let codex_row = breakdowns
            .iter()
            .find(|row| row.agent == "codex")
            .expect("Codex fixture should produce a row");
        assert_eq!(codex_row.period, "2026-07-22");
        assert_eq!(codex_row.input_tokens, 1000);
        assert_eq!(codex_row.output_tokens, 500);
        assert!((codex_row.total_cost - 0.006).abs() < f64::EPSILON);
        let pi_row = breakdowns
            .iter()
            .find(|row| row.agent == "pi")
            .expect("Pi fixture should produce a row");
        assert!((pi_row.total_cost - 0.01).abs() < f64::EPSILON);
    })
}

#[test]
fn loads_codex_with_source_pricing_speed_without_date_bounds() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "codex-home/sessions/2026/07/22/in-range.jsonl": codex_rollout(
            "2026-07-22",
            "gpt-source-config",
            1000,
            500,
        ),
    });
    isolated_source_environment(&fixture).run(|| {
        let shared = SharedArgs {
            mode: CostMode::Calculate,
            offline: true,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };
        let sources = BTreeMap::from([(
            "codex".to_string(),
            SourceArgs {
                options: ccusage_core::cli::AgentOptions {
                    codex_speed: CodexSpeed::Fast,
                    ..Default::default()
                },
                shared: SharedArgs {
                    pricing_overrides: BTreeMap::from([(
                        "gpt-source-config".to_string(),
                        crate::cli::PricingOverride {
                            input_cost_per_token: Some(1e-6),
                            output_cost_per_token: Some(2e-6),
                            fast_multiplier: Some(3.0),
                            ..Default::default()
                        },
                    )]),
                    ..shared.clone()
                },
            },
        )]);

        let result = load_rows(AgentReportKind::Daily, &shared, &sources, WeekDay::Monday).unwrap();

        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].input_tokens, 1000);
        assert_eq!(result.rows[0].output_tokens, 500);
        assert!((result.rows[0].total_cost - 0.006).abs() < f64::EPSILON);
    })
}

#[test]
fn reads_openclaw_entries_from_the_source_custom_path() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "custom-openclaw/agents/main/sessions/custom.jsonl": r#"{"type":"message","message":{"role":"assistant","model":"gpt-5.2","usage":{"input":1660,"output":55,"totalTokens":1715,"cost":{"total":0.02}},"timestamp":1769753935279}}"#,
    });
    isolated_source_environment(&fixture).run(|| {
        let shared = SharedArgs {
            mode: CostMode::Display,
            offline: true,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };
        let sources = BTreeMap::from([(
            "openclaw".to_string(),
            SourceArgs {
                options: ccusage_core::cli::AgentOptions {
                    open_claw_path: Some(
                        fixture
                            .path("custom-openclaw")
                            .to_string_lossy()
                            .into_owned(),
                    ),
                    ..Default::default()
                },
                shared: shared.clone(),
            },
        )]);

        let result = load_rows(AgentReportKind::Daily, &shared, &sources, WeekDay::Monday).unwrap();

        assert!(result.detected_agents.contains(&"openclaw"));
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].input_tokens, 1660);
        assert_eq!(result.rows[0].output_tokens, 55);
        assert!((result.rows[0].total_cost - 0.02).abs() < f64::EPSILON);
    })
}

#[test]
fn rejects_named_pi_store_overlapping_the_source_custom_path() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "custom/pi/project-a/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":30,"output":40}}}"#,
    });
    isolated_source_environment(&fixture).run(|| {
        let custom_pi_path = fixture.path("custom/pi").to_string_lossy().into_owned();
        let shared = SharedArgs {
            offline: true,
            since: Some("20990102".parse().unwrap()),
            until: Some("20990102".parse().unwrap()),
            pi_stores: vec![NamedPiStore {
                name: "fork".to_string(),
                path: custom_pi_path.clone(),
            }],
            ..SharedArgs::default()
        };
        let sources = BTreeMap::from([(
            "pi".to_string(),
            SourceArgs {
                options: ccusage_core::cli::AgentOptions {
                    pi_path: Some(custom_pi_path),
                    ..Default::default()
                },
                shared: shared.clone(),
            },
        )]);

        let Err(error) = load_rows(AgentReportKind::Daily, &shared, &sources, WeekDay::Monday)
        else {
            panic!("expected named pi store to collide with the configured pi path");
        };

        assert!(error.to_string().contains("Invalid ccusage config"));
        assert!(error.to_string().contains("fork"));
        assert!(error.to_string().contains("default pi store"));
    })
}

#[test]
fn rejects_named_pi_store_overlapping_the_source_agent_dir() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "agent/sessions/project-a/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":30,"output":40}}}"#,
    });
    isolated_source_environment(&fixture).run(|| {
        let shared = SharedArgs {
            offline: true,
            pi_stores: vec![NamedPiStore {
                name: "fork".to_string(),
                path: fixture
                    .path("agent/sessions")
                    .to_string_lossy()
                    .into_owned(),
            }],
            ..SharedArgs::default()
        };
        let sources = BTreeMap::from([(
            "pi".to_string(),
            SourceArgs {
                options: ccusage_core::cli::AgentOptions {
                    pi_agent_dir: Some(fixture.path("agent").to_string_lossy().into_owned()),
                    ..Default::default()
                },
                shared: shared.clone(),
            },
        )]);

        let Err(error) = load_rows(AgentReportKind::Daily, &shared, &sources, WeekDay::Monday)
        else {
            panic!("expected named pi store to collide with the configured pi agent directory");
        };

        assert!(error.to_string().contains("default pi store"));
    })
}

#[test]
fn named_pi_stores_keep_unified_shared_and_pricing_options() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "fork/sessions/project-a/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2099-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-named-source-config","usage":{"input":1000,"output":500}}}"#,
    });
    isolated_source_environment(&fixture).run(|| {
        let pricing_key = "[fork] gpt-named-source-config".to_string();
        let shared = SharedArgs {
            mode: CostMode::Calculate,
            offline: true,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            since: Some("20990102".parse().unwrap()),
            until: Some("20990102".parse().unwrap()),
            pi_stores: vec![NamedPiStore {
                name: "fork".to_string(),
                path: fixture.path("fork/sessions").to_string_lossy().into_owned(),
            }],
            pricing_overrides: BTreeMap::from([(
                pricing_key.clone(),
                crate::cli::PricingOverride {
                    input_cost_per_token: Some(1e-6),
                    output_cost_per_token: Some(2e-6),
                    ..Default::default()
                },
            )]),
            ..SharedArgs::default()
        };
        let sources = BTreeMap::from([(
            "pi".to_string(),
            SourceArgs {
                shared: SharedArgs {
                    since: Some("20990105".parse().unwrap()),
                    until: Some("20990106".parse().unwrap()),
                    pricing_overrides: BTreeMap::from([(
                        pricing_key,
                        crate::cli::PricingOverride {
                            input_cost_per_token: Some(10e-6),
                            output_cost_per_token: Some(20e-6),
                            ..Default::default()
                        },
                    )]),
                    ..shared.clone()
                },
                ..SourceArgs::default()
            },
        )]);

        let result = load_rows(AgentReportKind::Daily, &shared, &sources, WeekDay::Monday).unwrap();

        assert!(result.detected_agents.contains(&"fork"));
        let named_store = result.rows[0]
            .agent_breakdowns()
            .unwrap()
            .iter()
            .find(|row| row.agent == "fork")
            .expect("named store should use the unified date window");
        assert_eq!(named_store.input_tokens, 1000);
        assert_eq!(named_store.output_tokens, 500);
        assert!((named_store.total_cost - 0.002).abs() < f64::EPSILON);
    })
}

#[test]
fn named_pi_store_rows_are_additive_to_default_pi_rows() {
    let fixture = fs_fixture!({
        "pi/sessions/project-a/agent_pi-session.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":20}}}"#,
        "omp/sessions/project-b/agent_omp-session.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":30,"output":40}}}"#,
    });
    let shared = SharedArgs {
        mode: crate::cli::CostMode::Display,
        ..SharedArgs::default()
    };
    let mut rows = load_pi_format_agent_rows(
        Some(fixture.path("pi/sessions").to_str().unwrap()),
        AgentReportKind::Daily,
        &shared,
        &PricingMap::default(),
    )
    .unwrap()
    .rows;
    rows.extend(
        load_named_pi_store_rows(
            "omp",
            fixture.path("omp/sessions").to_str().unwrap(),
            AgentReportKind::Daily,
            &shared,
            &PricingMap::default(),
        )
        .unwrap()
        .rows,
    );

    let aggregated = aggregate_periods(rows, Bucket::Day).unwrap();

    assert_eq!(aggregated.len(), 1);
    assert_eq!(
        aggregated[0].agents().map(<[_]>::to_vec),
        Some(vec!["omp", "pi"])
    );
    let breakdowns = aggregated[0].agent_breakdowns().unwrap();
    assert_eq!(
        breakdowns
            .iter()
            .map(|row| (row.agent, row.models_used.as_slice()))
            .collect::<Vec<_>>(),
        vec![
            ("omp", ["[omp] gpt-5".to_string()].as_slice()),
            ("pi", ["[pi] gpt-5".to_string()].as_slice()),
        ]
    );
}
