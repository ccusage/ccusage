use super::accumulate::codex_period_for;
use super::intern::StringInterner;
use super::*;
use crate::parse_ts_timestamp;
use compact_str::CompactString;

use ccusage_test_support::fs_fixture;
use serde_json::json;

use crate::{
    CodexServiceTier, CodexSpeedPolicy, CodexUsageBucket, PricingMap,
    model_aliases::set_model_aliases_for_tests,
    paths::CodexUsageSource,
    test_support::{
        directory_source, load_groups_from_directory, load_groups_from_sources, load_rollouts,
        service_tier_line, token_count_line,
    },
};

#[test]
fn prices_the_configured_alias_target_when_both_models_have_rates() {
    ccusage_test_support::install_pricing_snapshots();
    let _aliases = set_model_aliases_for_tests([("logged-model", "alias-model")]);
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "logged-model":{"input_cost_per_token":1,"output_cost_per_token":1},
                "alias-model":{"input_cost_per_token":2,"output_cost_per_token":2}
            }"#,
    );
    let mut event = CodexTokenUsageEvent {
        session_id: "session".into(),
        response_id: None,
        record_response_id: None,
        timestamp: parse_ts_timestamp("2026-09-01T00:00:00Z").unwrap(),
        last_activity: "2026-09-01T00:00:00Z".into(),
        model: "logged-model".into(),
        input_tokens: 100,
        cached_input_tokens: 0,
        cache_creation_tokens: 0,
        output_tokens: 0,
        reasoning_output_tokens: 0,
        total_tokens: 100,
        is_fallback_model: false,
        service_tier: None,
    };
    let seen = CodexDedupe::new(1, Some(&pricing));
    let mut provenance = CodexEventProvenance::new(0);
    seen.record(&event, &mut provenance).unwrap();
    event.model = "alias-model".into();
    seen.record(&event, &mut provenance).unwrap();
    let groups = seen
        .fold(&SharedArgs::default(), AgentReportKind::Session)
        .unwrap();
    let group = &groups["session"];
    assert_eq!(
        group.models.keys().cloned().collect::<Vec<_>>(),
        ["alias-model"]
    );
    assert_eq!(group.input_tokens, 200);
    assert_eq!(
        crate::report::calculate_group_cost(
            group,
            Some(&pricing),
            CodexSpeedPolicy::Auto(CodexServiceTier::Standard)
        ),
        400.0,
    );
}

#[test]
fn applies_a_chained_alias_once_like_every_other_adapter() {
    ccusage_test_support::install_pricing_snapshots();
    let _aliases = set_model_aliases_for_tests([
        ("chain-first", "chain-second"),
        ("chain-second", "chain-third"),
    ]);
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "chain-first":{"input_cost_per_token":1,"output_cost_per_token":1},
                "chain-second":{"input_cost_per_token":2,"output_cost_per_token":2},
                "chain-third":{"input_cost_per_token":3,"output_cost_per_token":3}
            }"#,
    );
    let event = CodexTokenUsageEvent {
        session_id: "session".into(),
        response_id: None,
        record_response_id: None,
        timestamp: parse_ts_timestamp("2026-09-01T00:00:00Z").unwrap(),
        last_activity: "2026-09-01T00:00:00Z".into(),
        model: "chain-first".into(),
        input_tokens: 100,
        cached_input_tokens: 0,
        cache_creation_tokens: 0,
        output_tokens: 0,
        reasoning_output_tokens: 0,
        total_tokens: 100,
        is_fallback_model: false,
        service_tier: None,
    };
    let seen = CodexDedupe::new(1, Some(&pricing));
    let mut provenance = CodexEventProvenance::new(0);
    seen.record(&event, &mut provenance).unwrap();
    let groups = seen
        .fold(&SharedArgs::default(), AgentReportKind::Session)
        .unwrap();
    let group = &groups["session"];
    assert_eq!(
        group.models.keys().cloned().collect::<Vec<_>>(),
        ["chain-first"]
    );
    assert_eq!(
        crate::report::calculate_group_cost(
            group,
            Some(&pricing),
            CodexSpeedPolicy::Auto(CodexServiceTier::Standard)
        ),
        200.0,
    );
}

#[test]
fn does_not_price_an_unpriced_alias_target_through_its_own_alias() {
    ccusage_test_support::install_pricing_snapshots();
    let _aliases = set_model_aliases_for_tests([
        ("chain-own-first", "chain-own-second"),
        ("chain-own-second", "chain-own-third"),
    ]);
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "chain-own-first":{"input_cost_per_token":1,"output_cost_per_token":1},
                "chain-own-third":{"input_cost_per_token":3,"output_cost_per_token":3}
            }"#,
    );
    let event = CodexTokenUsageEvent {
        session_id: "session".into(),
        response_id: None,
        record_response_id: None,
        timestamp: parse_ts_timestamp("2026-09-01T00:00:00Z").unwrap(),
        last_activity: "2026-09-01T00:00:00Z".into(),
        model: "chain-own-first".into(),
        input_tokens: 100,
        cached_input_tokens: 0,
        cache_creation_tokens: 0,
        output_tokens: 0,
        reasoning_output_tokens: 0,
        total_tokens: 100,
        is_fallback_model: false,
        service_tier: None,
    };
    let seen = CodexDedupe::new(1, Some(&pricing));
    let mut provenance = CodexEventProvenance::new(0);
    seen.record(&event, &mut provenance).unwrap();
    let groups = seen
        .fold(&SharedArgs::default(), AgentReportKind::Session)
        .unwrap();
    let group = &groups["session"];
    assert_eq!(
        group.models.keys().cloned().collect::<Vec<_>>(),
        ["chain-own-first"]
    );
    assert_eq!(
        crate::report::calculate_group_cost(
            group,
            Some(&pricing),
            CodexSpeedPolicy::Auto(CodexServiceTier::Standard)
        ),
        100.0,
    );
}

#[test]
fn groups_under_the_alias_target_whose_fuzzy_row_priced_the_usage() {
    ccusage_test_support::install_pricing_snapshots();
    let _aliases =
        set_model_aliases_for_tests([("codex-unpriced-logged", "claude-codex-family-9-thinking")]);
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{"claude-codex-family-9":{"input_cost_per_token":7,"output_cost_per_token":7}}"#,
    );
    let event = CodexTokenUsageEvent {
        session_id: "session".into(),
        response_id: None,
        record_response_id: None,
        timestamp: parse_ts_timestamp("2026-09-01T00:00:00Z").unwrap(),
        last_activity: "2026-09-01T00:00:00Z".into(),
        model: "codex-unpriced-logged".into(),
        input_tokens: 100,
        cached_input_tokens: 0,
        cache_creation_tokens: 0,
        output_tokens: 0,
        reasoning_output_tokens: 0,
        total_tokens: 100,
        is_fallback_model: false,
        service_tier: None,
    };
    let seen = CodexDedupe::new(1, Some(&pricing));
    let mut provenance = CodexEventProvenance::new(0);
    seen.record(&event, &mut provenance).unwrap();
    let groups = seen
        .fold(&SharedArgs::default(), AgentReportKind::Session)
        .unwrap();
    let group = &groups["session"];
    assert_eq!(
        group.models.keys().cloned().collect::<Vec<_>>(),
        ["claude-codex-family-9-thinking"]
    );
    assert_eq!(
        crate::report::calculate_group_cost(
            group,
            Some(&pricing),
            CodexSpeedPolicy::Auto(CodexServiceTier::Standard)
        ),
        700.0,
    );
}

#[test]
fn bills_a_fuzzy_priced_alias_target_once_even_when_it_has_its_own_alias() {
    ccusage_test_support::install_pricing_snapshots();
    let _aliases = set_model_aliases_for_tests([
        ("chain-hop-a", "claude-chain-hop-9-thinking"),
        ("claude-chain-hop-9-thinking", "chain-hop-c"),
    ]);
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "claude-chain-hop-9":{"input_cost_per_token":2,"output_cost_per_token":2},
                "chain-hop-c":{"input_cost_per_token":3,"output_cost_per_token":3}
            }"#,
    );
    assert_eq!(pricing.find("chain-hop-a").unwrap().input, 2.0);
    let event = CodexTokenUsageEvent {
        session_id: "session".into(),
        response_id: None,
        record_response_id: None,
        timestamp: parse_ts_timestamp("2026-09-01T00:00:00Z").unwrap(),
        last_activity: "2026-09-01T00:00:00Z".into(),
        model: "chain-hop-a".into(),
        input_tokens: 100,
        cached_input_tokens: 0,
        cache_creation_tokens: 0,
        output_tokens: 0,
        reasoning_output_tokens: 0,
        total_tokens: 100,
        is_fallback_model: false,
        service_tier: None,
    };
    let seen = CodexDedupe::new(1, Some(&pricing));
    let mut provenance = CodexEventProvenance::new(0);
    seen.record(&event, &mut provenance).unwrap();
    let groups = seen
        .fold(&SharedArgs::default(), AgentReportKind::Session)
        .unwrap();
    let group = &groups["session"];
    assert_eq!(
        group.models.keys().cloned().collect::<Vec<_>>(),
        ["chain-hop-a"]
    );
    assert_eq!(
        crate::report::calculate_group_cost(
            group,
            Some(&pricing),
            CodexSpeedPolicy::Auto(CodexServiceTier::Standard)
        ),
        200.0,
    );
}

#[test]
fn keeps_differently_priced_requests_in_separate_groups() {
    ccusage_test_support::install_pricing_snapshots();
    let _aliases = set_model_aliases_for_tests([
        ("group-a-source", "group-b-target"),
        ("group-b-target", "group-c-target"),
    ]);
    let mut pricing = PricingMap::default();
    pricing.load_json(r#"{"group-b-target":{"input_cost_per_token":2,"output_cost_per_token":2}}"#);
    let key = "group-c-target".to_owned();
    let rates = ccusage_core::cli::PricingOverride {
        input_cost_per_token: Some(5.0),
        output_cost_per_token: Some(5.0),
        ..Default::default()
    };
    let pricing = pricing.with_user_overrides([(&key, &rates)]);
    for order in [
        ["group-a-source", "group-b-target"],
        ["group-b-target", "group-a-source"],
    ] {
        let seen = CodexDedupe::new(1, Some(&pricing));
        let mut provenance = CodexEventProvenance::new(0);
        for (index, model) in order.into_iter().enumerate() {
            let event = CodexTokenUsageEvent {
                session_id: "session".into(),
                response_id: None,
                record_response_id: None,
                timestamp: parse_ts_timestamp(&format!("2026-09-01T00:00:0{index}Z")).unwrap(),
                last_activity: "2026-09-01T00:00:00Z".into(),
                model: model.into(),
                input_tokens: 100,
                cached_input_tokens: 0,
                cache_creation_tokens: 0,
                output_tokens: 0,
                reasoning_output_tokens: 0,
                total_tokens: 100,
                is_fallback_model: false,
                service_tier: None,
            };
            seen.record(&event, &mut provenance).unwrap();
        }
        let groups = seen
            .fold(&SharedArgs::default(), AgentReportKind::Session)
            .unwrap();
        let group = &groups["session"];
        assert_eq!(group.models.len(), 2, "{order:?}");
        assert_eq!(
            crate::report::calculate_group_cost(
                group,
                Some(&pricing),
                CodexSpeedPolicy::Auto(CodexServiceTier::Standard)
            ),
            700.0,
            "{order:?}"
        );
    }
}

#[test]
fn tier_split_uses_the_supplied_pricing_entry_instead_of_embedded_rates() {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    pricing.load_json(r#"{"gpt-5.5":{"input_cost_per_token":0.000001,"output_cost_per_token":0.000002,"input_cost_per_token_above_200k_tokens":0.000003}}"#);
    let event = CodexTokenUsageEvent {
        session_id: "session".into(),
        response_id: None,
        record_response_id: None,
        timestamp: parse_ts_timestamp("2026-09-01T00:00:00Z").unwrap(),
        last_activity: "2026-09-01T00:00:00Z".into(),
        model: "gpt-5.5".into(),
        input_tokens: 250_000,
        cached_input_tokens: 0,
        cache_creation_tokens: 0,
        output_tokens: 10,
        reasoning_output_tokens: 0,
        total_tokens: 250_010,
        is_fallback_model: false,
        service_tier: None,
    };
    let seen = CodexDedupe::new(1, Some(&pricing));
    seen.record(&event, &mut CodexEventProvenance::new(0))
        .unwrap();
    let groups = seen
        .fold(&SharedArgs::default(), AgentReportKind::Session)
        .unwrap();
    assert_eq!(
        groups["session"].models["gpt-5.5"]
            .usage
            .long_context_input_tokens,
        250_000
    );
}

#[test]
fn saturates_usage_totals_across_distinct_requests() {
    ccusage_test_support::install_pricing_snapshots();
    let line = token_count_line(
        "2026-09-01T08:00:00Z",
        "deepseek-v4-flash",
        json!({
            "input_tokens": u64::MAX,
            "cached_input_tokens": 1,
            "cache_write_input_tokens": 1,
            "output_tokens": u64::MAX,
            "reasoning_output_tokens": 1,
            "total_tokens": u64::MAX,
        }),
    );
    let rollout = [
        service_tier_line("2026-09-01T07:59:00Z", "priority"),
        line.clone(),
        line,
    ];
    let groups = load_rollouts(&[("session.jsonl", &rollout)], AgentReportKind::Daily);
    let group = &groups["2026-09-01"];
    let usage = &group.models["deepseek-v4-flash"];
    assert_eq!(group.input_tokens, u64::MAX);
    assert_eq!(group.output_tokens, u64::MAX);
    assert_eq!(group.total_tokens, u64::MAX);
    assert_eq!(usage.usage.input_tokens, u64::MAX);
    assert_eq!(usage.usage.output_tokens, u64::MAX);
    assert_eq!(usage.total_tokens, u64::MAX);
    assert_eq!(
        usage.recorded_usage[crate::CodexServiceTier::Fast].input_tokens,
        u64::MAX
    );
    assert_eq!(
        usage.recorded_usage[crate::CodexServiceTier::Fast].cached_input_tokens,
        2
    );
    assert_eq!(
        usage
            .timestamped_usage
            .values()
            .next()
            .unwrap()
            .usage
            .input_tokens,
        u64::MAX
    );
}

#[test]
fn saturates_each_usage_bucket_component() {
    let event = crate::CodexRawUsage {
        input_tokens: u64::MAX,
        cached_input_tokens: u64::MAX,
        cache_creation_tokens: u64::MAX,
        output_tokens: u64::MAX,
        ..crate::CodexRawUsage::default()
    };
    let mut usage = CodexUsageBucket::default();
    usage += CodexUsageBucket::from_usage(event, true);
    usage += CodexUsageBucket::from_usage(event, true);
    assert_eq!(
        usage,
        CodexUsageBucket {
            input_tokens: u64::MAX,
            cached_input_tokens: u64::MAX,
            cache_creation_tokens: u64::MAX,
            output_tokens: u64::MAX,
            long_context_input_tokens: u64::MAX,
            long_context_cached_input_tokens: u64::MAX,
            long_context_cache_creation_tokens: u64::MAX,
            long_context_output_tokens: u64::MAX,
        }
    );
}

#[test]
fn usage_attribution_uses_file_order_when_workers_finish_in_reverse() {
    ccusage_test_support::install_pricing_snapshots();
    let first = CodexTokenUsageEvent {
        session_id: "session".into(),
        response_id: None,
        record_response_id: None,
        timestamp: crate::parse_ts_timestamp("2026-09-01T00:00:01Z").unwrap(),
        last_activity: "2026-09-01T00:00:01Z".into(),
        model: "gpt-5".into(),
        input_tokens: 300,
        cached_input_tokens: 0,
        cache_creation_tokens: 0,
        output_tokens: 20,
        reasoning_output_tokens: 0,
        total_tokens: 320,
        is_fallback_model: false,
        service_tier: Some(CodexServiceTier::Standard),
    };
    let mut copied = first.clone();
    copied.last_activity = "2026-09-01T00:00:01.000Z".into();
    copied.is_fallback_model = true;
    copied.service_tier = Some(CodexServiceTier::Fast);
    let shared = SharedArgs {
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };
    let pricing = PricingMap::load_embedded();
    let seen = CodexDedupe::new(2, Some(&pricing));
    for (event, file_index) in [(&copied, 1), (&first, 0)] {
        seen.record(event, &mut CodexEventProvenance::new(file_index))
            .unwrap();
    }
    let groups = seen.fold(&shared, AgentReportKind::Session).unwrap();

    assert_eq!(groups.len(), 1);
    let group = &groups["session"];
    assert_eq!(group.total_tokens, 320);
    assert_eq!(group.last_activity.as_deref(), Some("2026-09-01T00:00:01Z"));
    assert!(!group.models["gpt-5"].is_fallback);
    assert_eq!(
        group.models["gpt-5"].recorded_usage[crate::CodexServiceTier::Standard].input_tokens,
        300
    );
}

#[test]
fn compaction_attribution_uses_file_order_when_workers_finish_in_reverse() {
    ccusage_test_support::install_pricing_snapshots();
    let first = CodexTokenUsageEvent {
        session_id: "first-session".into(),
        response_id: Some("response-1".into()),
        record_response_id: None,
        timestamp: crate::parse_ts_timestamp("2026-09-01T00:00:01Z").unwrap(),
        last_activity: "2026-09-01T00:00:01Z".into(),
        model: "gpt-5".into(),
        input_tokens: 300,
        cached_input_tokens: 0,
        cache_creation_tokens: 0,
        output_tokens: 20,
        reasoning_output_tokens: 0,
        total_tokens: 320,
        is_fallback_model: true,
        service_tier: Some(CodexServiceTier::Standard),
    };
    let mut copied = first.clone();
    copied.session_id = "copied-session".into();
    copied.timestamp = crate::parse_ts_timestamp("2026-09-02T00:00:01Z").unwrap();
    copied.last_activity = "2026-09-02T00:00:01Z".into();
    copied.model = "gpt-5-mini".into();
    copied.input_tokens = 500;
    copied.total_tokens = 520;
    copied.service_tier = Some(CodexServiceTier::Fast);
    let shared = SharedArgs {
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };
    let pricing = PricingMap::load_embedded();
    let seen = CodexDedupe::new(2, Some(&pricing));
    for (event, file_index) in [(&copied, 1), (&first, 0)] {
        seen.record(event, &mut CodexEventProvenance::new(file_index))
            .unwrap();
    }
    let groups = seen.fold(&shared, AgentReportKind::Session).unwrap();

    assert_eq!(groups.len(), 1);
    let group = &groups["first-session"];
    assert_eq!(group.total_tokens, 320);
    assert_eq!(group.last_activity.as_deref(), Some("2026-09-01T00:00:01Z"));
    assert_eq!(
        group.models["gpt-5"].recorded_usage[crate::CodexServiceTier::Standard].input_tokens,
        300
    );
}

#[test]
fn selects_codex_period_for_each_report_kind() {
    let timestamp = parse_ts_timestamp("2026-05-29T08:01:00.000Z").unwrap();
    let shared = SharedArgs {
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };
    let mut sessions = StringInterner::default();
    let session = sessions
        .intern("sessions/child.jsonl", || {
            CompactString::new("sessions/child.jsonl")
        })
        .unwrap();

    for (kind, expected) in [
        (AgentReportKind::Daily, "2026-05-29"),
        (AgentReportKind::Weekly, "2026-05-25"),
        (AgentReportKind::Monthly, "2026-05"),
        (AgentReportKind::Session, "sessions/child.jsonl"),
    ] {
        assert_eq!(
            codex_period_for(
                timestamp,
                session,
                kind,
                &ccusage_core::ReportWindow::from_shared(&shared),
            )
            .map(|period| period.label(kind, &sessions))
            .as_deref(),
            Some(expected),
        );
    }
}

#[test]
fn stores_timestamped_usage_only_for_time_dependent_models() {
    ccusage_test_support::install_pricing_snapshots();
    let line = |model: &str| {
        token_count_line(
            "2026-08-17T01:00:00.000Z",
            model,
            json!({"input_tokens": 1_000_000, "output_tokens": 0, "total_tokens": 1_000_000}),
        )
    };
    let groups = load_rollouts(
        &[(
            "session-1.jsonl",
            &[line("gpt-5"), line("deepseek-v4-flash")],
        )],
        AgentReportKind::Daily,
    );
    let models = &groups["2026-08-17"].models;

    assert!(models["gpt-5"].timestamped_usage.is_empty());
    assert_eq!(models["deepseek-v4-flash"].timestamped_usage.len(), 1);
}

#[test]
fn records_flex_usage_in_model_buckets() {
    ccusage_test_support::install_pricing_snapshots();
    let rollout = [
        service_tier_line("2026-08-17T00:59:00.000Z", "flex"),
        token_count_line(
            "2026-08-17T01:00:00.000Z",
            "gpt-test",
            json!({
                "input_tokens": 1_000,
                "cached_input_tokens": 100,
                "cache_write_input_tokens": 50,
                "output_tokens": 25,
                "reasoning_output_tokens": 5,
                "total_tokens": 1_025,
            }),
        ),
    ];

    let groups = load_rollouts(&[("session-1.jsonl", &rollout)], AgentReportKind::Daily);
    let usage = &groups["2026-08-17"].models["gpt-test"];

    assert_eq!(usage.usage.input_tokens, 1_000);
    assert_eq!(
        usage.recorded_usage[crate::CodexServiceTier::Flex].input_tokens,
        1_000
    );
    assert_eq!(
        usage.recorded_usage[crate::CodexServiceTier::Flex].cached_input_tokens,
        100
    );
    assert_eq!(
        usage.recorded_usage[crate::CodexServiceTier::Flex].cache_creation_tokens,
        50
    );
    assert_eq!(
        usage.recorded_usage[crate::CodexServiceTier::Flex].output_tokens,
        25
    );
}

#[test]
fn keys_every_day_of_a_month_to_one_monthly_period() {
    let window = ccusage_core::ReportWindow::from_shared(&SharedArgs {
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    });
    let session = StringInterner::default()
        .intern("session", || CompactString::new("session"))
        .unwrap();
    let monthly = |timestamp| {
        codex_period_for(
            parse_ts_timestamp(timestamp).unwrap(),
            session,
            AgentReportKind::Monthly,
            &window,
        )
    };

    assert_eq!(
        monthly("2026-05-01T00:00:00.000Z"),
        monthly("2026-05-31T23:59:59.000Z")
    );
    assert_ne!(
        monthly("2026-05-31T23:59:59.000Z"),
        monthly("2026-06-01T00:00:00.000Z")
    );
}

#[test]
fn omits_codex_period_outside_date_bounds() {
    let shared = SharedArgs {
        since: Some("20260528".parse().unwrap()),
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        until: Some("20260530".parse().unwrap()),
        ..SharedArgs::default()
    };
    let session = StringInterner::default()
        .intern("sessions/child.jsonl", || {
            CompactString::new("sessions/child.jsonl")
        })
        .unwrap();

    for timestamp in ["2026-05-27T23:59:59.000Z", "2026-05-31T00:00:00.000Z"] {
        assert_eq!(
            codex_period_for(
                parse_ts_timestamp(timestamp).unwrap(),
                session,
                AgentReportKind::Daily,
                &ccusage_core::ReportWindow::from_shared(&shared),
            ),
            None,
        );
    }
}

#[test]
fn skips_historical_files_before_parsing_but_keeps_long_running_sessions() {
    ccusage_test_support::install_pricing_snapshots();
    let usage_line = |timestamp: &str, input_tokens: u64| {
        json!({
            "timestamp": timestamp,
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "model": "gpt-5",
                    "last_token_usage": {
                        "input_tokens": input_tokens,
                        "cached_input_tokens": 0,
                        "output_tokens": 1,
                        "total_tokens": input_tokens + 1,
                    },
                },
            },
        })
        .to_string()
    };
    let historical = [
        json!({
            "timestamp": "2025-01-01T08:00:00.000Z",
            "type": "session_meta",
            "payload": {"id": "historical"},
        })
        .to_string(),
        usage_line("2026-03-15T08:01:00.000Z", 999),
        r#"{"timestamp":"2026-03-15T08:02:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"secret":"#.to_string(),
    ]
    .join("\n");
    let long_running = [
        json!({
            "timestamp": "2026-01-01T08:00:00.000Z",
            "type": "session_meta",
            "payload": {"id": "long-running"},
        })
        .to_string(),
        usage_line("2026-01-01T08:01:00.000Z", 10),
        usage_line("2026-03-15T08:01:00.000Z", 100),
    ]
    .join("\n");
    let fixture = fs_fixture!({
        "sessions/2025/01/01/historical.jsonl": &historical,
        "sessions/2026/01/01/long-running.jsonl": &long_running,
    });
    let historical_path = fixture.path("sessions/2025/01/01/historical.jsonl");
    let long_running_path = fixture.path("sessions/2026/01/01/long-running.jsonl");
    crate::test_support::set_file_modified(
        &historical_path,
        parse_ts_timestamp("2025-01-01T08:01:00.000Z").unwrap(),
    );
    crate::test_support::set_file_modified(
        &long_running_path,
        parse_ts_timestamp("2026-03-15T08:01:00.000Z").unwrap(),
    );
    let shared = SharedArgs {
        since: Some("20260315".parse().unwrap()),
        until: Some("20260315".parse().unwrap()),
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };

    let load = |shared: &SharedArgs| {
        let diagnostics = Diagnostics::new("codex");
        let (groups, _) = load_groups_from_sources_with_diagnostics(
            &[directory_source(&fixture.path("sessions"))],
            shared,
            AgentReportKind::Daily,
            Some(&PricingMap::load_embedded()),
            &diagnostics,
        )
        .unwrap();
        (
            groups,
            diagnostics.skipped(ccusage_core::SkipKind::InvalidData),
        )
    };

    for single_thread in [true, false] {
        let (bounded, bounded_skips) = load(&SharedArgs {
            single_thread,
            ..shared.clone()
        });
        let group = &bounded["2026-03-15"];
        assert_eq!(group.input_tokens, 100);
        assert_eq!(group.output_tokens, 1);
        assert_eq!(group.total_tokens, 101);
        assert_eq!(
            bounded_skips, 0,
            "the historical file was read past its first line"
        );

        let (unbounded, unbounded_skips) = load(&SharedArgs {
            single_thread,
            ..SharedArgs::default()
        });
        assert_eq!(unbounded_skips, 1);
        assert_eq!(unbounded["2026-03-15"].input_tokens, 1_099);
    }
}

#[test]
fn retains_next_utc_path_day_for_local_until_boundary() {
    ccusage_test_support::install_pricing_snapshots();
    let late_utc = [
        json!({
            "timestamp": "2026-03-16T06:29:00.000Z",
            "type": "session_meta",
            "payload": {"id": "late-utc"},
        })
        .to_string(),
        json!({
            "timestamp": "2026-03-16T06:30:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "model": "gpt-5",
                    "last_token_usage": {
                        "input_tokens": 100,
                        "cached_input_tokens": 0,
                        "output_tokens": 1,
                        "total_tokens": 101,
                    },
                },
            },
        })
        .to_string(),
    ]
    .join("\n");
    let fixture = fs_fixture!({
        "sessions/2026/03/16/late-utc.jsonl": &late_utc,
    });
    let shared = SharedArgs {
        since: Some("20260315".parse().unwrap()),
        until: Some("20260315".parse().unwrap()),
        timezone: Some("America/Los_Angeles".parse().unwrap()),
        single_thread: true,
        ..SharedArgs::default()
    };

    let groups =
        load_groups_from_directory(&fixture.path("sessions"), &shared, AgentReportKind::Daily)
            .unwrap();

    assert_eq!(groups["2026-03-15"].input_tokens, 100);
    assert_eq!(groups["2026-03-15"].total_tokens, 101);
}

#[test]
fn dedupes_copied_token_usage_across_session_files() {
    ccusage_test_support::install_pricing_snapshots();
    let usage_line = json!({
        "timestamp": "2026-05-29T08:01:00.000Z",
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": {
                "model": "gpt-5.2",
                "last_token_usage": {
                    "input_tokens": 1_000,
                    "cached_input_tokens": 100,
                    "output_tokens": 200,
                    "reasoning_output_tokens": 20,
                    "total_tokens": 1_200,
                },
            },
        },
    })
    .to_string();
    let service_tier_line = json!({
        "timestamp": "2026-05-29T08:00:00.000Z",
        "type": "event_msg",
        "payload": {
            "type": "thread_settings_applied",
            "thread_settings": { "service_tier": "priority" },
        },
    })
    .to_string();
    let rollout = format!("{service_tier_line}\n{usage_line}");
    let unclassified_first = fs_fixture!({
        "sessions/a-unclassified.jsonl": &usage_line,
        "sessions/z-recorded.jsonl": &rollout,
    });
    let recorded_first = fs_fixture!({
        "sessions/a-recorded.jsonl": &rollout,
        "sessions/z-unclassified.jsonl": &usage_line,
    });
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "gpt-5.2": {
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000002,
                    "cache_read_input_token_cost": 0.0000005,
                    "provider_specific_entry": { "fast": 2 }
                }
            }"#,
    );
    let mut costs = Vec::new();
    for fixture in [&unclassified_first, &recorded_first] {
        for single_thread in [true, false] {
            let shared = SharedArgs {
                single_thread,
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                ..SharedArgs::default()
            };

            let groups = load_groups_from_directory(
                &fixture.path("sessions"),
                &shared,
                AgentReportKind::Daily,
            )
            .unwrap();

            assert_eq!(groups.len(), 1);
            let group = groups.get("2026-05-29").unwrap();
            assert_eq!(group.input_tokens, 1_000);
            assert_eq!(group.cached_input_tokens, 100);
            assert_eq!(group.output_tokens, 200);
            assert_eq!(group.reasoning_output_tokens, 20);
            assert_eq!(group.total_tokens, 1_200);
            assert_eq!(
                group.models["gpt-5.2"].recorded_usage[crate::CodexServiceTier::Fast].input_tokens,
                1_000
            );
            costs.push(crate::calculate_group_cost(
                group,
                Some(&pricing),
                CodexSpeedPolicy::Auto(CodexServiceTier::Standard),
            ));
        }
    }
    assert!(
        costs
            .windows(2)
            .all(|pair| pair[0].to_bits() == pair[1].to_bits())
    );
}

#[test]
fn resolves_conflicting_duplicate_tiers_as_standard() {
    ccusage_test_support::install_pricing_snapshots();
    let usage_line = json!({
        "timestamp": "2026-05-29T08:01:00.000Z",
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": {
                "model": "gpt-5.2",
                "last_token_usage": {
                    "input_tokens": 1_000,
                    "cached_input_tokens": 100,
                    "output_tokens": 200,
                    "total_tokens": 1_200,
                },
            },
        },
    })
    .to_string();
    let rollout = |service_tier: &str| {
        format!(
            "{}\n{usage_line}",
            json!({
                "timestamp": "2026-05-29T08:00:00.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "thread_settings_applied",
                    "thread_settings": { "service_tier": service_tier },
                },
            })
        )
    };
    let standard = rollout("default");
    let fast = rollout("priority");
    let standard_first = fs_fixture!({
        "sessions/a-standard.jsonl": &standard,
        "sessions/z-fast.jsonl": &fast,
    });
    let fast_first = fs_fixture!({
        "sessions/a-fast.jsonl": &fast,
        "sessions/z-standard.jsonl": &standard,
    });

    for fixture in [&standard_first, &fast_first] {
        for single_thread in [true, false] {
            let shared = SharedArgs {
                single_thread,
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                ..SharedArgs::default()
            };
            let groups = load_groups_from_directory(
                &fixture.path("sessions"),
                &shared,
                AgentReportKind::Daily,
            )
            .unwrap();
            let usage = &groups["2026-05-29"].models["gpt-5.2"];

            assert_eq!(usage.usage.input_tokens, 1_000);
            assert_eq!(
                usage.recorded_usage[crate::CodexServiceTier::Standard].input_tokens,
                1_000
            );
            assert_eq!(
                usage.recorded_usage[crate::CodexServiceTier::Fast].input_tokens,
                0
            );
        }
    }
}

#[test]
fn tracks_long_context_token_split_per_request() {
    ccusage_test_support::install_pricing_snapshots();
    let usage_line = |input: u64, cached: u64, output: u64| {
        json!({
            "timestamp": "2026-07-09T08:01:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "model": "gpt-5.6-sol",
                    "last_token_usage": {
                        "input_tokens": input,
                        "cached_input_tokens": cached,
                        "output_tokens": output,
                        "reasoning_output_tokens": 0,
                        "total_tokens": input + output,
                    },
                },
            },
        })
        .to_string()
    };
    let long_line = usage_line(280_000, 20_000, 500);
    let short_line = usage_line(100_000, 50_000, 300);
    let fast_marker = json!({
        "timestamp": "2026-07-09T08:00:00.000Z",
        "type": "event_msg",
        "payload": {
            "type": "thread_settings_applied",
            "thread_settings": { "service_tier": "priority" },
        },
    })
    .to_string();
    let standard_marker = json!({
        "timestamp": "2026-07-09T08:02:00.000Z",
        "type": "event_msg",
        "payload": {
            "type": "thread_settings_applied",
            "thread_settings": { "service_tier": "default" },
        },
    })
    .to_string();
    let fixture = fs_fixture!({
        "sessions/root.jsonl": &format!(
            "{fast_marker}\n{long_line}\n{standard_marker}\n{short_line}"
        ),
    });
    let shared = SharedArgs {
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };

    let groups =
        load_groups_from_directory(&fixture.path("sessions"), &shared, AgentReportKind::Daily)
            .unwrap();

    let group = groups.get("2026-07-09").unwrap();
    let usage = group.models.get("gpt-5.6-sol").unwrap();
    assert_eq!(usage.usage.input_tokens, 380_000);
    assert_eq!(usage.usage.cached_input_tokens, 70_000);
    assert_eq!(usage.usage.output_tokens, 800);
    assert_eq!(usage.usage.long_context_input_tokens, 280_000);
    assert_eq!(usage.usage.long_context_cached_input_tokens, 20_000);
    assert_eq!(usage.usage.long_context_output_tokens, 500);
    assert_eq!(
        usage.recorded_usage[crate::CodexServiceTier::Fast].input_tokens,
        280_000
    );
    assert_eq!(
        usage.recorded_usage[crate::CodexServiceTier::Fast].cached_input_tokens,
        20_000
    );
    assert_eq!(
        usage.recorded_usage[crate::CodexServiceTier::Fast].output_tokens,
        500
    );
    assert_eq!(
        usage.recorded_usage[crate::CodexServiceTier::Fast].long_context_input_tokens,
        280_000
    );
    assert_eq!(
        usage.recorded_usage[crate::CodexServiceTier::Fast].long_context_cached_input_tokens,
        20_000
    );
    assert_eq!(
        usage.recorded_usage[crate::CodexServiceTier::Fast].long_context_output_tokens,
        500
    );
    assert_eq!(
        usage.recorded_usage[crate::CodexServiceTier::Standard].input_tokens,
        100_000
    );
    assert_eq!(
        usage.recorded_usage[crate::CodexServiceTier::Standard].cached_input_tokens,
        50_000
    );
    assert_eq!(
        usage.recorded_usage[crate::CodexServiceTier::Standard].output_tokens,
        300
    );
}

#[test]
fn parallel_merge_preserves_speed_and_long_context_buckets() {
    ccusage_test_support::install_pricing_snapshots();
    let rollout = |marker_timestamp: &str,
                   usage_timestamp: &str,
                   service_tier: &str,
                   input_tokens: u64,
                   cached_input_tokens: u64,
                   cache_creation_tokens: u64,
                   output_tokens: u64| {
        [
            json!({
                "timestamp": marker_timestamp,
                "type": "event_msg",
                "payload": {
                    "type": "thread_settings_applied",
                    "thread_settings": { "service_tier": service_tier },
                },
            })
            .to_string(),
            json!({
                "timestamp": usage_timestamp,
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "model": "gpt-5.6-sol",
                        "last_token_usage": {
                            "input_tokens": input_tokens,
                            "cached_input_tokens": cached_input_tokens,
                            "cache_write_input_tokens": cache_creation_tokens,
                            "output_tokens": output_tokens,
                            "total_tokens": input_tokens + output_tokens,
                        },
                    },
                },
            })
            .to_string(),
        ]
        .join("\n")
    };
    let fast_long = rollout(
        "2026-07-09T08:00:00.000Z",
        "2026-07-09T08:01:00.000Z",
        "priority",
        280_000,
        20_000,
        30_000,
        500,
    );
    let standard_short = rollout(
        "2026-07-09T09:00:00.000Z",
        "2026-07-09T09:01:00.000Z",
        "default",
        100_000,
        50_000,
        10_000,
        300,
    );
    let fixture = fs_fixture!({
        "sessions/fast.jsonl": &fast_long,
        "sessions/standard.jsonl": &standard_short,
    });
    let mut observed = Vec::new();

    for single_thread in [true, false] {
        let shared = SharedArgs {
            single_thread,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };
        let groups =
            load_groups_from_directory(&fixture.path("sessions"), &shared, AgentReportKind::Daily)
                .unwrap();
        let usage = &groups["2026-07-09"].models["gpt-5.6-sol"];

        assert_eq!(usage.usage.input_tokens, 380_000);
        assert_eq!(usage.usage.cache_creation_tokens, 40_000);
        assert_eq!(usage.usage.long_context_input_tokens, 280_000);
        assert_eq!(usage.usage.long_context_cache_creation_tokens, 30_000);
        assert_eq!(
            usage.recorded_usage[crate::CodexServiceTier::Fast].input_tokens,
            280_000
        );
        assert_eq!(
            usage.recorded_usage[crate::CodexServiceTier::Fast].cache_creation_tokens,
            30_000
        );
        assert_eq!(
            usage.recorded_usage[crate::CodexServiceTier::Fast].long_context_input_tokens,
            280_000
        );
        assert_eq!(
            usage.recorded_usage[crate::CodexServiceTier::Fast].long_context_cache_creation_tokens,
            30_000
        );
        assert_eq!(
            usage.recorded_usage[crate::CodexServiceTier::Standard].input_tokens,
            100_000
        );
        assert_eq!(
            usage.recorded_usage[crate::CodexServiceTier::Standard].cache_creation_tokens,
            10_000
        );
        assert_eq!(
            usage.recorded_usage[crate::CodexServiceTier::Standard].long_context_input_tokens,
            0
        );
        observed.push((
            usage.recorded_usage[crate::CodexServiceTier::Fast],
            usage.recorded_usage[crate::CodexServiceTier::Standard],
        ));
    }

    assert_eq!(observed[0], observed[1]);
}

#[test]
fn folds_cache_creation_usage_into_groups_and_recorded_buckets() {
    ccusage_test_support::install_pricing_snapshots();
    let rollout = [
        service_tier_line("2026-07-09T08:00:00Z", "default"),
        token_count_line(
            "2026-07-09T08:01:00Z",
            "gpt-5.6-sol",
            json!({
                "input_tokens": 280_000,
                "cache_write_input_tokens": 30,
                "output_tokens": 1,
            }),
        ),
    ];
    let groups = load_rollouts(&[("session.jsonl", &rollout)], AgentReportKind::Daily);
    let group = &groups["2026-07-09"];
    let usage = &group.models["gpt-5.6-sol"];
    assert_eq!(group.cache_creation_tokens, 30);
    assert_eq!(usage.usage.cache_creation_tokens, 30);
    assert_eq!(
        usage.recorded_usage[crate::CodexServiceTier::Standard].long_context_cache_creation_tokens,
        30
    );
}

#[test]
fn dedupes_copied_token_usage_after_model_alias_resolution() {
    ccusage_test_support::install_pricing_snapshots();
    let _aliases = set_model_aliases_for_tests([("private-alpha", "gpt-5.2")]);
    let private_usage_line = json!({
        "timestamp": "2026-05-29T08:01:00.000Z",
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": {
                "model": "private-alpha",
                "last_token_usage": {
                    "input_tokens": 1_000,
                    "cached_input_tokens": 100,
                    "output_tokens": 200,
                    "reasoning_output_tokens": 20,
                    "total_tokens": 1_200,
                },
            },
        },
    })
    .to_string();
    let canonical_usage_line = json!({
        "timestamp": "2026-05-29T08:01:00.000Z",
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": {
                "model": "gpt-5.2",
                "last_token_usage": {
                    "input_tokens": 1_000,
                    "cached_input_tokens": 100,
                    "output_tokens": 200,
                    "reasoning_output_tokens": 20,
                    "total_tokens": 1_200,
                },
            },
        },
    })
    .to_string();
    let fixture = fs_fixture!({
        "sessions/root.jsonl": &private_usage_line,
        "sessions/goal.jsonl": &canonical_usage_line,
    });
    for single_thread in [true, false] {
        let shared = SharedArgs {
            single_thread,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };

        let groups =
            load_groups_from_directory(&fixture.path("sessions"), &shared, AgentReportKind::Daily)
                .unwrap();

        let group = groups.get("2026-05-29").unwrap();
        assert_eq!(group.input_tokens, 1_000);
        assert_eq!(group.models.len(), 1);
        assert_eq!(group.models["gpt-5.2"].usage.input_tokens, 1_000);
    }
}

#[test]
fn credits_a_recordless_copy_to_one_session() {
    ccusage_test_support::install_pricing_snapshots();
    let usage_line = json!({
        "timestamp": "2026-05-29T08:01:00.000Z",
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": {
                "model": "gpt-5.2",
                "last_token_usage": {
                    "input_tokens": 1_000,
                    "cached_input_tokens": 100,
                    "output_tokens": 200,
                    "reasoning_output_tokens": 20,
                    "total_tokens": 1_200,
                },
            },
        },
    })
    .to_string();
    let fixture = fs_fixture!({
        "sessions/root.jsonl": &usage_line,
        "sessions/goal.jsonl": &usage_line,
    });
    for single_thread in [true, false] {
        let shared = SharedArgs {
            single_thread,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };

        let groups = load_groups_from_directory(
            &fixture.path("sessions"),
            &shared,
            AgentReportKind::Session,
        )
        .unwrap();

        assert_eq!(groups.len(), 1);
        assert_eq!(groups["goal"].input_tokens, 1_000);
    }
}

#[test]
fn aggregates_active_copy_when_archived_file_has_same_relative_path() {
    ccusage_test_support::install_pricing_snapshots();
    let active_usage = [
        json!({
            "timestamp": "2026-05-12T08:00:00.000Z",
            "type": "turn_context",
            "payload": {
                "model": "gpt-5.2",
            },
        })
        .to_string(),
        json!({
            "timestamp": "2026-05-12T08:01:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "total_token_usage": {
                        "input_tokens": 111,
                        "cached_input_tokens": 10,
                        "output_tokens": 20,
                        "reasoning_output_tokens": 1,
                        "total_tokens": 131,
                    },
                },
            },
        })
        .to_string(),
    ]
    .join("\n");
    let archived_usage = [
        json!({
            "timestamp": "2026-05-12T09:00:00.000Z",
            "type": "turn_context",
            "payload": {
                "model": "gpt-5.2",
            },
        })
        .to_string(),
        json!({
            "timestamp": "2026-05-12T09:01:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "total_token_usage": {
                        "input_tokens": 999,
                        "cached_input_tokens": 90,
                        "output_tokens": 80,
                        "reasoning_output_tokens": 7,
                        "total_tokens": 1_079,
                    },
                },
            },
        })
        .to_string(),
    ]
    .join("\n");
    let fixture = fs_fixture!({
        "codex/sessions/duplicate.jsonl": active_usage,
        "codex/archived_sessions/duplicate.jsonl": archived_usage,
        "codex/archived_sessions/archived-only.jsonl": [
            json!({
                "timestamp": "2026-05-13T08:00:00.000Z",
                "type": "turn_context",
                "payload": {
                    "model": "gpt-5.2",
                },
            })
            .to_string(),
            json!({
                "timestamp": "2026-05-13T08:01:00.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {
                            "input_tokens": 222,
                            "cached_input_tokens": 20,
                            "output_tokens": 30,
                            "reasoning_output_tokens": 2,
                            "total_tokens": 252,
                        },
                    },
                },
            })
            .to_string(),
        ]
        .join("\n"),
    });

    for single_thread in [true, false] {
        let shared = SharedArgs {
            single_thread,
            ..SharedArgs::default()
        };
        let sources = vec![
            CodexUsageSource::new(fixture.path("codex/sessions"), fixture.path("codex")),
            CodexUsageSource::new(
                fixture.path("codex/archived_sessions"),
                fixture.path("codex"),
            ),
        ];
        let groups = load_groups_from_sources(&sources, &shared, AgentReportKind::Daily).unwrap();

        assert_eq!(groups.len(), 2);
        assert_eq!(groups["2026-05-12"].input_tokens, 111);
        assert_eq!(groups["2026-05-13"].input_tokens, 222);
    }
}

fn fork_token_count_line(timestamp: &str, last: u64, total: u64) -> String {
    json!({
        "timestamp": timestamp,
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": {
                "model": "gpt-5.2",
                "last_token_usage": {"input_tokens": last, "output_tokens": 1, "total_tokens": last + 1},
                "total_token_usage": {"input_tokens": total, "output_tokens": 1, "total_tokens": total + 1},
            },
        },
    })
    .to_string()
}

#[test]
fn skips_replayed_history_of_forks_read_during_the_pass() {
    ccusage_test_support::install_pricing_snapshots();
    let parent = [
        json!({
            "timestamp": "2026-05-12T08:00:00.000Z",
            "type": "session_meta",
            "payload": {"id": "parent-xyz"},
        })
        .to_string(),
        fork_token_count_line("2026-05-12T08:01:00.000Z", 1_000, 1_000),
        fork_token_count_line("2026-05-12T08:02:00.000Z", 500, 1_500),
    ]
    .join("\n");
    let fork = [
        json!({
            "timestamp": "2026-05-12T08:03:00.000Z",
            "type": "session_meta",
            "payload": {"id": "fork-abc", "forked_from_id": "parent-xyz"},
        })
        .to_string(),
        fork_token_count_line("2026-05-12T08:03:00.000Z", 1_000, 1_000),
        fork_token_count_line("2026-05-12T08:03:00.000Z", 500, 1_500),
        fork_token_count_line("2026-05-12T08:04:00.000Z", 100, 1_600),
    ]
    .join("\n");
    let fixture = fs_fixture!({
        "codex/sessions/2026/05/12/rollout-2026-05-12T08-00-00-parent-xyz.jsonl": parent.clone(),
        "codex/archived_sessions/rollout-2026-05-12T08-03-00-fork-abc.jsonl": fork.clone(),
        "single/2026/05/12/rollout-2026-05-12T08-00-00-parent-xyz.jsonl": parent,
        "single/2026/05/12/rollout-2026-05-12T08-03-00-fork-abc.jsonl": fork,
    });
    let sources = [
        CodexUsageSource::new(fixture.path("codex/sessions"), fixture.path("codex")),
        CodexUsageSource::new(
            fixture.path("codex/archived_sessions"),
            fixture.path("codex"),
        ),
    ];

    for single_thread in [true, false] {
        for since in [None, Some("20260101".parse().unwrap())] {
            let shared = SharedArgs {
                single_thread,
                since,
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                ..SharedArgs::default()
            };
            let context = format!("single_thread={single_thread} since={:?}", shared.since);
            let sessions = |groups: BTreeMap<String, CodexGroup>| {
                groups
                    .into_iter()
                    .map(|(session, group)| (session, group.input_tokens))
                    .collect::<Vec<_>>()
            };
            let by_sources =
                load_groups_from_sources(&sources, &shared, AgentReportKind::Session).unwrap();
            assert_eq!(
                sessions(by_sources),
                [
                    (
                        "2026/05/12/rollout-2026-05-12T08-00-00-parent-xyz".to_string(),
                        1_500
                    ),
                    ("rollout-2026-05-12T08-03-00-fork-abc".to_string(), 100),
                ],
                "{context}"
            );
            let by_directory = load_groups_from_directory(
                &fixture.path("single"),
                &shared,
                AgentReportKind::Session,
            )
            .unwrap();
            assert_eq!(
                sessions(by_directory),
                [
                    (
                        "2026/05/12/rollout-2026-05-12T08-00-00-parent-xyz".to_string(),
                        1_500
                    ),
                    (
                        "2026/05/12/rollout-2026-05-12T08-03-00-fork-abc".to_string(),
                        100
                    ),
                ],
                "{context}"
            );
        }
    }
}

fn session_meta_line(timestamp: &str, id: &str, parent: Option<&str>) -> String {
    let mut payload = json!({"id": id});
    if let Some(parent) = parent {
        payload["forked_from_id"] = parent.into();
    }
    json!({"timestamp": timestamp, "type": "session_meta", "payload": payload}).to_string()
}

fn input_tokens_by_period(
    sessions_dir: &Path,
    kind: AgentReportKind,
    single_thread: bool,
) -> Vec<(String, u64)> {
    let shared = SharedArgs {
        single_thread,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };
    load_groups_from_directory(sessions_dir, &shared, kind)
        .unwrap()
        .into_iter()
        .map(|(period, group)| (period, group.input_tokens))
        .collect()
}

const PARENT_ROLLOUT: &str = "2026/05/12/rollout-2026-05-12T08-00-00-parent-xyz";
const FORK_ROLLOUT: &str = "2026/05/12/rollout-2026-05-12T08-30-00-fork-abc";

fn parent_and_fork_requesting_at_the_same_instant() -> [String; 2] {
    let parent = [
        session_meta_line("2026-05-12T08:00:00.000Z", "parent-xyz", None),
        fork_token_count_line("2026-05-12T08:01:00.000Z", 1_000, 1_000),
        fork_token_count_line("2026-05-12T09:00:00.000Z", 100, 1_100),
    ];
    let fork = [
        session_meta_line("2026-05-12T08:30:00.000Z", "fork-abc", Some("parent-xyz")),
        fork_token_count_line("2026-05-12T08:30:00.000Z", 1_000, 1_000),
        fork_token_count_line("2026-05-12T09:00:00.000Z", 100, 1_100),
    ];
    [parent.join("\n"), fork.join("\n")]
}

#[test]
fn counts_a_fork_and_its_parent_requesting_at_the_same_instant_twice() {
    ccusage_test_support::install_pricing_snapshots();
    let [parent, fork] = parent_and_fork_requesting_at_the_same_instant();
    let fixture = fs_fixture!({
        "sessions/2026/05/12/rollout-2026-05-12T08-00-00-parent-xyz.jsonl": parent,
        "sessions/2026/05/12/rollout-2026-05-12T08-30-00-fork-abc.jsonl": fork,
    });
    let sessions = fixture.path("sessions");

    for single_thread in [true, false] {
        assert_eq!(
            input_tokens_by_period(&sessions, AgentReportKind::Daily, single_thread),
            [("2026-05-12".to_string(), 1_200)],
            "single_thread={single_thread}"
        );
        assert_eq!(
            input_tokens_by_period(&sessions, AgentReportKind::Session, single_thread),
            [
                (PARENT_ROLLOUT.to_string(), 1_100),
                (FORK_ROLLOUT.to_string(), 100),
            ],
            "single_thread={single_thread}"
        );
    }
}

#[test]
fn counts_a_copy_without_fork_metadata_once_in_every_report() {
    ccusage_test_support::install_pricing_snapshots();
    let request = fork_token_count_line("2026-05-12T09:00:00.000Z", 100, 1_100);
    let fixture = fs_fixture!({
        "sessions/a-original.jsonl": session_meta_line("2026-05-12T08:00:00.000Z", "original", None) + "\n" + &request,
        "sessions/b-copy.jsonl": session_meta_line("2026-05-12T08:00:00.000Z", "copy", None) + "\n" + &request,
    });
    let sessions = fixture.path("sessions");

    for single_thread in [true, false] {
        assert_eq!(
            input_tokens_by_period(&sessions, AgentReportKind::Daily, single_thread),
            [("2026-05-12".to_string(), 100)],
            "single_thread={single_thread}"
        );
        assert_eq!(
            input_tokens_by_period(&sessions, AgentReportKind::Session, single_thread),
            [("a-original".to_string(), 100)],
            "single_thread={single_thread}"
        );
    }
}

#[test]
fn collapses_a_parent_copy_while_keeping_the_fork_request() {
    ccusage_test_support::install_pricing_snapshots();
    let [parent, fork] = parent_and_fork_requesting_at_the_same_instant();
    let copy = fork_token_count_line("2026-05-12T09:00:00.000Z", 100, 1_100);
    let fixture = fs_fixture!({
        "sessions/0-copy.jsonl": copy,
        "sessions/2026/05/12/rollout-2026-05-12T08-00-00-parent-xyz.jsonl": parent,
        "sessions/2026/05/12/rollout-2026-05-12T08-30-00-fork-abc.jsonl": fork,
    });
    let sessions = fixture.path("sessions");

    for single_thread in [true, false] {
        assert_eq!(
            input_tokens_by_period(&sessions, AgentReportKind::Daily, single_thread),
            [("2026-05-12".to_string(), 1_200)],
            "single_thread={single_thread}"
        );
        assert_eq!(
            input_tokens_by_period(&sessions, AgentReportKind::Session, single_thread),
            [
                ("0-copy".to_string(), 100),
                (PARENT_ROLLOUT.to_string(), 1_000),
                (FORK_ROLLOUT.to_string(), 100),
            ],
            "single_thread={single_thread}"
        );
    }
}

fn fork_pair(parent: &str, fork: &str, prefix: u64) -> [String; 2] {
    let request = fork_token_count_line("2026-05-12T09:00:00.000Z", 100, prefix + 100);
    [
        [
            session_meta_line("2026-05-12T08:00:00.000Z", parent, None),
            fork_token_count_line("2026-05-12T08:01:00.000Z", prefix, prefix),
            request.clone(),
        ]
        .join("\n"),
        [
            session_meta_line("2026-05-12T08:30:00.000Z", fork, Some(parent)),
            fork_token_count_line("2026-05-12T08:30:00.000Z", prefix, prefix),
            request,
        ]
        .join("\n"),
    ]
}

#[test]
fn a_copy_of_an_ancestor_does_not_add_a_request_across_fork_pairs() {
    ccusage_test_support::install_pricing_snapshots();
    let [parent_a, fork_b] = fork_pair("parent-a", "fork-b", 1_000);
    let [parent_c, fork_d] = fork_pair("parent-c", "fork-d", 2_000);

    for with_copy in [false, true] {
        let fixture = fs_fixture!({
            "sessions/a.jsonl": &parent_a,
            "sessions/b.jsonl": &fork_b,
            "sessions/c.jsonl": &parent_c,
            "sessions/d.jsonl": &fork_d,
        });
        if with_copy {
            fixture.write_file("sessions/archived-a.jsonl", &parent_a);
        }
        let sessions = fixture.path("sessions");

        for single_thread in [true, false] {
            let context = format!("with_copy={with_copy} single_thread={single_thread}");
            assert_eq!(
                input_tokens_by_period(&sessions, AgentReportKind::Daily, single_thread),
                [("2026-05-12".to_string(), 3_200)],
                "{context}"
            );
            let by_session =
                input_tokens_by_period(&sessions, AgentReportKind::Session, single_thread);
            assert_eq!(
                by_session.iter().map(|(_, input)| input).sum::<u64>(),
                3_200,
                "{context}"
            );
        }
    }
}

#[test]
fn counts_a_reference_backed_fork_and_its_parent_requesting_at_the_same_instant_twice() {
    ccusage_test_support::install_pricing_snapshots();
    let request = fork_token_count_line("2026-05-12T09:00:00.000Z", 100, 100);
    let fork_meta = json!({
        "timestamp": "2026-05-12T08:30:00.000Z",
        "type": "session_meta",
        "payload": {
            "id": "fork-abc",
            "forked_from_id": "parent-xyz",
            "history_base": {"thread_id": "parent-xyz", "end_ordinal_exclusive": 1},
        },
    })
    .to_string();
    let fixture = fs_fixture!({
        "sessions/parent.jsonl": session_meta_line("2026-05-12T08:00:00.000Z", "parent-xyz", None) + "\n" + &request,
        "sessions/fork.jsonl": fork_meta + "\n" + &request,
    });
    let sessions = fixture.path("sessions");

    for single_thread in [true, false] {
        assert_eq!(
            input_tokens_by_period(&sessions, AgentReportKind::Daily, single_thread),
            [("2026-05-12".to_string(), 200)],
            "single_thread={single_thread}"
        );
        assert_eq!(
            input_tokens_by_period(&sessions, AgentReportKind::Session, single_thread),
            [("fork".to_string(), 100), ("parent".to_string(), 100)],
            "single_thread={single_thread}"
        );
    }
}

fn bounded_input_tokens(
    sessions_dir: &Path,
    kind: AgentReportKind,
    single_thread: bool,
    bounded: bool,
) -> Vec<(String, u64)> {
    let day = bounded.then(|| "20260512".parse().unwrap());
    let shared = SharedArgs {
        single_thread,
        since: day,
        until: day,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };
    load_groups_from_directory(sessions_dir, &shared, kind)
        .unwrap()
        .into_iter()
        .filter(|(period, _)| kind == AgentReportKind::Session || period == "2026-05-12")
        .map(|(period, group)| (period, group.input_tokens))
        .collect()
}

#[test]
fn bounded_loads_keep_lineage_through_an_out_of_window_ancestor() {
    for reference_backed_leaf in [false, true] {
        assert_bounded_lineage_through_an_out_of_window_ancestor(reference_backed_leaf);
    }
}

fn assert_bounded_lineage_through_an_out_of_window_ancestor(reference_backed_leaf: bool) {
    ccusage_test_support::install_pricing_snapshots();
    let request = fork_token_count_line("2026-05-12T09:00:00.000Z", 100, 1_150);
    let root = [
        session_meta_line("2026-05-01T08:00:00.000Z", "root-a", None),
        fork_token_count_line("2026-05-01T08:01:00.000Z", 1_000, 1_000),
        request.clone(),
    ];
    let middle = [
        session_meta_line("2026-05-01T09:00:00.000Z", "middle-b", Some("root-a")),
        fork_token_count_line("2026-05-01T09:00:00.000Z", 1_000, 1_000),
        fork_token_count_line("2026-05-01T10:00:00.000Z", 50, 1_050),
    ];
    let leaf = if reference_backed_leaf {
        vec![
            json!({
                "timestamp": "2026-05-12T08:00:00.000Z",
                "type": "session_meta",
                "payload": {
                    "id": "leaf-c",
                    "forked_from_id": "middle-b",
                    "history_base": {"thread_id": "middle-b", "end_ordinal_exclusive": 2},
                },
            })
            .to_string(),
            request,
        ]
    } else {
        vec![
            session_meta_line("2026-05-12T08:00:00.000Z", "leaf-c", Some("middle-b")),
            fork_token_count_line("2026-05-12T08:00:00.000Z", 1_000, 1_000),
            fork_token_count_line("2026-05-12T08:00:00.000Z", 50, 1_050),
            request,
        ]
    };
    let fixture = fs_fixture!({
        "sessions/2026/05/01/rollout-2026-05-01T08-00-00-root-a.jsonl": root.join("\n"),
        "sessions/2026/05/01/rollout-2026-05-01T09-00-00-middle-b.jsonl": middle.join("\n"),
        "sessions/2026/05/12/rollout-2026-05-12T08-00-00-leaf-c.jsonl": leaf.join("\n"),
    });
    let modified = |path: &str, at: &str| {
        ccusage_test_support::set_modified(&fixture.path(path), ccusage_test_support::instant(at));
    };
    modified(
        "sessions/2026/05/01/rollout-2026-05-01T08-00-00-root-a.jsonl",
        "2026-05-12T09:01:00Z",
    );
    modified(
        "sessions/2026/05/01/rollout-2026-05-01T09-00-00-middle-b.jsonl",
        "2026-05-01T10:01:00Z",
    );
    let sessions = fixture.path("sessions");

    for single_thread in [true, false] {
        for bounded in [false, true] {
            let context = format!(
                "reference_backed_leaf={reference_backed_leaf} single_thread={single_thread} bounded={bounded}"
            );
            assert_eq!(
                bounded_input_tokens(&sessions, AgentReportKind::Daily, single_thread, bounded),
                [("2026-05-12".to_string(), 200)],
                "{context}"
            );
        }
        assert_eq!(
            bounded_input_tokens(&sessions, AgentReportKind::Session, single_thread, true),
            [
                (
                    "2026/05/01/rollout-2026-05-01T08-00-00-root-a".to_string(),
                    100
                ),
                (
                    "2026/05/12/rollout-2026-05-12T08-00-00-leaf-c".to_string(),
                    100
                ),
            ],
            "reference_backed_leaf={reference_backed_leaf} single_thread={single_thread}"
        );
    }
}

const IDENTICAL_REQUESTS_ROLLOUT: &str = r#"{"timestamp":"2026-11-01T09:42:09.923Z","type":"session_meta","payload":{"id":"01a1967c-0000-7000-8000-00000000000a","forked_from_id":"01a1967b-0000-7000-8000-000000000001","timestamp":"2026-11-01T09:42:09.923Z","originator":"codex_vscode","cli_version":"0.164.0-alpha.11.1","source":{"subagent":{"thread_spawn":{"parent_thread_id":"01a1967b-0000-7000-8000-000000000001","depth":1}}},"model_provider":"openai"}}
{"timestamp":"2026-11-01T09:42:09.923Z","type":"compacted","payload":{"message":"","replacement_history":[]}}
{"timestamp":"2026-11-01T09:42:09.923Z","type":"event_msg","payload":{"type":"thread_settings_applied","thread_id":"01a1967b-0000-7000-8000-000000000001","thread_settings":{}}}
{"timestamp":"2026-11-01T09:42:09.923Z","type":"turn_context","payload":{"model":"gpt-6-astra"}}
{"timestamp":"2026-11-01T09:42:09.923Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":990000,"cached_input_tokens":0,"output_tokens":10000,"reasoning_output_tokens":0,"total_tokens":1000000},"last_token_usage":{"input_tokens":990000,"cached_input_tokens":0,"output_tokens":10000,"reasoning_output_tokens":0,"total_tokens":1000000}}}}
{"timestamp":"2026-11-01T09:42:09.923Z","type":"token_usage_record","payload":{"thread_id":"01a1967c-0000-7000-8000-00000000000a","response_id":"resp_child_1","usage":{"input_tokens":10000,"cached_input_tokens":2000,"output_tokens":100,"reasoning_output_tokens":0,"total_tokens":10100}}}
{"timestamp":"2026-11-01T09:42:09.923Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000000,"cached_input_tokens":2000,"output_tokens":10100,"reasoning_output_tokens":0,"total_tokens":1010100},"last_token_usage":{"input_tokens":10000,"cached_input_tokens":2000,"output_tokens":100,"reasoning_output_tokens":0,"total_tokens":10100}}}}
{"timestamp":"2026-11-01T09:42:09.923Z","type":"token_usage_record","payload":{"thread_id":"01a1967c-0000-7000-8000-00000000000a","response_id":"resp_child_2","usage":{"input_tokens":10000,"cached_input_tokens":2000,"output_tokens":100,"reasoning_output_tokens":0,"total_tokens":10100}}}
{"timestamp":"2026-11-01T09:42:09.923Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1010000,"cached_input_tokens":4000,"output_tokens":10200,"reasoning_output_tokens":0,"total_tokens":1020200},"last_token_usage":{"input_tokens":10000,"cached_input_tokens":2000,"output_tokens":100,"reasoning_output_tokens":0,"total_tokens":10100}}}}"#;

fn identical_request_line() -> String {
    json!({
        "timestamp": "2026-11-01T09:00:00.000Z",
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": {
                "model": "gpt-5.2",
                "last_token_usage": {
                    "input_tokens": 1_000,
                    "cached_input_tokens": 100,
                    "output_tokens": 10,
                    "total_tokens": 1_010,
                },
            },
        },
    })
    .to_string()
}

fn daily_input_tokens(sessions_dir: &Path, single_thread: bool) -> Vec<(String, u64)> {
    let shared = SharedArgs {
        single_thread,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };
    load_groups_from_directory(sessions_dir, &shared, AgentReportKind::Daily)
        .unwrap()
        .into_iter()
        .map(|(period, group)| (period, group.input_tokens))
        .collect()
}

#[test]
fn counts_identical_requests_within_one_rollout() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "sessions/2026/11/01/rollout-2026-11-01T09-42-09-01a1967c-0000-7000-8000-00000000000a.jsonl":
            IDENTICAL_REQUESTS_ROLLOUT,
    });
    let sessions = fixture.path("sessions");

    for single_thread in [true, false] {
        let events =
            crate::test_support::rollout_events_with_single_thread(&sessions, single_thread)
                .into_iter()
                .map(|event| {
                    (
                        event.input_tokens,
                        event.cached_input_tokens,
                        event.output_tokens,
                    )
                })
                .collect::<Vec<_>>();
        assert_eq!(
            events,
            [(10_000, 2_000, 100), (10_000, 2_000, 100)],
            "single_thread={single_thread}"
        );

        let shared = SharedArgs {
            single_thread,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };
        let groups =
            load_groups_from_directory(&sessions, &shared, AgentReportKind::Session).unwrap();
        let totals = groups
            .values()
            .map(|group| {
                (
                    group.input_tokens - group.cached_input_tokens,
                    group.cached_input_tokens,
                    group.output_tokens,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            totals,
            [(16_000, 4_000, 200)],
            "single_thread={single_thread}"
        );
    }
}

#[test]
fn counts_each_copied_request_once_across_rollouts() {
    ccusage_test_support::install_pricing_snapshots();
    let request = identical_request_line();
    let both_requests = format!("{request}\n{request}");
    let scenarios = [
        ("copy of both requests", both_requests.clone()),
        ("copy of the first request", request.clone()),
    ];

    for (name, copy) in scenarios {
        let fixture = fs_fixture!({
            "sessions/a-original.jsonl": &both_requests,
            "sessions/b-copy.jsonl": &copy,
        });
        let sessions = fixture.path("sessions");

        for single_thread in [true, false] {
            assert_eq!(
                daily_input_tokens(&sessions, single_thread),
                [("2026-11-01".to_string(), 2_000)],
                "{name}, single_thread={single_thread}"
            );
        }
    }
}

#[test]
fn counts_spawned_siblings_with_identical_usage_and_dedupes_copied_records() {
    ccusage_test_support::install_pricing_snapshots();
    let sibling = IDENTICAL_REQUESTS_ROLLOUT
        .replace("00000000000a", "00000000000b")
        .replace("resp_child_1", "resp_child_3")
        .replace("resp_child_2", "resp_child_4");
    let copy = IDENTICAL_REQUESTS_ROLLOUT.replace("00000000000a", "00000000000c");
    let scenarios = [
        ("distinct sibling", sibling, (32_000, 8_000, 400, 40_400)),
        ("copied records", copy, (16_000, 4_000, 200, 20_200)),
    ];

    for (name, second, expected) in scenarios {
        let fixture = fs_fixture!({
            "sessions/2026/11/01/rollout-2026-11-01T09-42-09-01a1967c-0000-7000-8000-00000000000a.jsonl":
                IDENTICAL_REQUESTS_ROLLOUT,
            "sessions/2026/11/01/rollout-2026-11-01T09-42-09-01a1967c-0000-7000-8000-00000000000b.jsonl":
                &second,
        });
        let sessions = fixture.path("sessions");

        for single_thread in [true, false] {
            let shared = SharedArgs {
                single_thread,
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                ..SharedArgs::default()
            };
            let groups =
                load_groups_from_directory(&sessions, &shared, AgentReportKind::Daily).unwrap();
            let group = &groups["2026-11-01"];
            assert_eq!(
                (
                    group.input_tokens - group.cached_input_tokens,
                    group.cached_input_tokens,
                    group.output_tokens,
                    group.total_tokens,
                ),
                expected,
                "{name}, single_thread={single_thread}"
            );
        }
    }
}

fn recorded_requests(service_tier: Option<&str>, response_ids: &[Option<&str>]) -> String {
    let mut lines = vec![
        json!({"timestamp": "2026-11-01T09:00:00.000Z", "type": "turn_context", "payload": {"model": "gpt-5.2"}})
            .to_string(),
    ];
    if let Some(service_tier) = service_tier {
        lines.push(
            json!({"timestamp": "2026-11-01T09:00:00.000Z", "type": "event_msg", "payload": {"type": "thread_settings_applied", "thread_settings": {"service_tier": service_tier}}})
                .to_string(),
        );
    }
    let timestamp = "2026-11-01T09:01:00.000Z";
    for (index, response_id) in response_ids.iter().enumerate() {
        let usage = json!({"input_tokens": 1_000, "cached_input_tokens": 100, "output_tokens": 10, "total_tokens": 1_010});
        let total = (u64::try_from(index).unwrap() + 1) * 1_000;
        if let Some(response_id) = response_id {
            lines.push(
                json!({"timestamp": timestamp, "type": "token_usage_record", "payload": {"response_id": response_id, "usage": usage}})
                    .to_string(),
            );
        }
        lines.push(
            json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": {"type": "token_count", "info": {
                    "last_token_usage": usage,
                    "total_token_usage": {"input_tokens": total, "cached_input_tokens": total / 10, "output_tokens": total / 100, "total_tokens": total + total / 100},
                }},
            })
            .to_string(),
        );
    }
    lines.join("\n")
}

#[test]
fn matches_recordless_copies_to_recorded_requests_in_either_file_order() {
    ccusage_test_support::install_pricing_snapshots();
    let original = recorded_requests(None, &[Some("resp_a1"), Some("resp_a2")]);
    let sibling = recorded_requests(None, &[Some("resp_b1"), Some("resp_b2")]);
    let copy = recorded_requests(None, &[None, None]);
    let partial = recorded_requests(None, &[Some("resp_a2")]);
    let scenarios = [
        ("recorded first", vec![&original, &copy], 2),
        ("recordless first", vec![&copy, &original], 2),
        ("siblings, then copy", vec![&original, &sibling, &copy], 4),
        ("copy between siblings", vec![&original, &copy, &sibling], 4),
        (
            "copy first, then siblings",
            vec![&copy, &original, &sibling],
            4,
        ),
        ("partial copy last", vec![&original, &partial], 2),
        ("partial copy first", vec![&partial, &original], 2),
        (
            "partial and recordless copies",
            vec![&partial, &copy, &original],
            2,
        ),
    ];

    for (name, files, event_count) in scenarios {
        let fixture = ccusage_test_support::Fixture::new();
        for (index, contents) in files.iter().enumerate() {
            fixture.write_file(format!("sessions/{index}-rollout.jsonl"), contents);
        }
        let sessions = fixture.path("sessions");

        for single_thread in [true, false] {
            assert_eq!(
                daily_input_tokens(&sessions, single_thread),
                [("2026-11-01".to_string(), 1_000 * event_count)],
                "{name}, single_thread={single_thread}"
            );
        }
    }
}

#[test]
fn keeps_recorded_tiers_when_a_recordless_copy_cannot_name_its_request() {
    ccusage_test_support::install_pricing_snapshots();
    let fast = recorded_requests(Some("priority"), &[Some("resp_fast")]);
    let flex = recorded_requests(Some("flex"), &[Some("resp_flex")]);
    let copy = recorded_requests(Some("priority"), &[None]);
    let orders = [
        [&fast, &flex, &copy],
        [&fast, &copy, &flex],
        [&flex, &fast, &copy],
        [&flex, &copy, &fast],
        [&copy, &fast, &flex],
        [&copy, &flex, &fast],
    ];

    for (order, files) in orders.iter().enumerate() {
        let fixture = ccusage_test_support::Fixture::new();
        for (index, contents) in files.iter().enumerate() {
            fixture.write_file(format!("sessions/{index}-rollout.jsonl"), contents);
        }
        let sessions = fixture.path("sessions");

        for single_thread in [true, false] {
            let shared = SharedArgs {
                single_thread,
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                ..SharedArgs::default()
            };
            let groups =
                load_groups_from_directory(&sessions, &shared, AgentReportKind::Daily).unwrap();
            let usage = &groups["2026-11-01"].models["gpt-5.2"];
            assert_eq!(
                (
                    usage.usage.input_tokens,
                    usage.recorded_usage[CodexServiceTier::Fast].input_tokens,
                    usage.recorded_usage[CodexServiceTier::Flex].input_tokens,
                ),
                (2_000, 1_000, 1_000),
                "order={order}, single_thread={single_thread}"
            );
        }
    }
}
#[test]
fn credits_a_replaced_stand_in_to_the_recording_session() {
    ccusage_test_support::install_pricing_snapshots();
    let child = recorded_requests(None, &[None]);
    let parent = recorded_requests(None, &[Some("resp_parent")]);
    let orders = [
        [("0-child", &child), ("1-parent", &parent)],
        [("0-parent", &parent), ("1-child", &child)],
    ];

    for files in orders {
        let fixture = ccusage_test_support::Fixture::new();
        for (name, contents) in files {
            fixture.write_file(format!("sessions/{name}.jsonl"), contents);
        }
        let sessions = fixture.path("sessions");
        let parent_session = files
            .iter()
            .find(|(name, _)| name.ends_with("-parent"))
            .unwrap()
            .0;

        for single_thread in [true, false] {
            let shared = SharedArgs {
                single_thread,
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                since: Some("20261101".parse().unwrap()),
                until: Some("20261101".parse().unwrap()),
                ..SharedArgs::default()
            };
            let (groups, _) = load_groups_from_sources_with_diagnostics(
                &[directory_source(&sessions)],
                &shared,
                AgentReportKind::Session,
                None,
                &Diagnostics::new("codex"),
            )
            .unwrap();
            assert_eq!(
                groups
                    .into_iter()
                    .map(|(session, group)| (session, group.input_tokens))
                    .collect::<Vec<_>>(),
                [(parent_session.to_string(), 1_000)],
                "parent={parent_session}, single_thread={single_thread}"
            );
            assert_eq!(
                daily_input_tokens(&sessions, single_thread),
                [("2026-11-01".to_string(), 1_000)],
                "parent={parent_session}, single_thread={single_thread}"
            );
        }
    }
}

#[test]
fn replaces_the_stand_in_attribution_regardless_of_arrival_order() {
    let stand_in = CodexTokenUsageEvent {
        session_id: "session-b".into(),
        response_id: None,
        record_response_id: None,
        timestamp: parse_ts_timestamp("2026-11-01T09:00:00Z").unwrap(),
        last_activity: "2026-11-01T09:00:00Z".into(),
        model: "gpt-5.2".into(),
        input_tokens: 100,
        cached_input_tokens: 0,
        cache_creation_tokens: 0,
        output_tokens: 0,
        reasoning_output_tokens: 0,
        total_tokens: 100,
        is_fallback_model: true,
        service_tier: Some(CodexServiceTier::Fast),
    };
    let recorded = CodexTokenUsageEvent {
        session_id: "session-a".into(),
        record_response_id: Some("resp-a".into()),
        is_fallback_model: false,
        service_tier: Some(CodexServiceTier::Flex),
        last_activity: "2026-11-01T09:00:00.000Z".into(),
        ..stand_in.clone()
    };
    let events = [stand_in, recorded];
    for order in [[0, 1], [1, 0]] {
        let seen = CodexDedupe::new(2, None);
        for file_index in order {
            seen.record(
                &events[file_index],
                &mut CodexEventProvenance::new(file_index),
            )
            .unwrap();
        }
        let groups = seen
            .fold(&SharedArgs::default(), AgentReportKind::Session)
            .unwrap();
        assert_eq!(
            groups.keys().map(String::as_str).collect::<Vec<_>>(),
            ["session-a"]
        );
        let group = &groups["session-a"];
        assert_eq!(group.input_tokens, 100);
        assert_eq!(
            group.last_activity.as_deref(),
            Some("2026-11-01T09:00:00.000Z")
        );
        let model = &group.models["gpt-5.2"];
        assert!(!model.is_fallback);
        assert_eq!(
            model.recorded_usage[CodexServiceTier::Flex].input_tokens,
            100
        );
    }
}

#[test]
fn preserves_recordless_requests_that_cooccur_with_their_recorded_match() {
    ccusage_test_support::install_pricing_snapshots();
    let recorded = recorded_requests(Some("priority"), &[Some("resp_x")]);
    let mixed_after = recorded_requests(Some("standard"), &[Some("resp_x"), None]);
    let mixed_before = recorded_requests(Some("standard"), &[None, Some("resp_x")]);
    let recordless = recorded_requests(Some("standard"), &[None, None]);
    for mixed in [&mixed_after, &mixed_before] {
        for files in [
            vec![&recorded, mixed],
            vec![mixed, &recorded],
            vec![&recorded, &recordless, mixed],
            vec![mixed, &recorded, &recordless],
            vec![&recordless, mixed, &recorded],
        ] {
            let fixture = ccusage_test_support::Fixture::new();
            for (index, contents) in files.iter().enumerate() {
                fixture.write_file(format!("sessions/{index}.jsonl"), contents);
            }
            let sessions = fixture.path("sessions");
            for single_thread in [true, false] {
                assert_eq!(
                    daily_input_tokens(&sessions, single_thread),
                    [("2026-11-01".to_string(), 2_000)],
                    "single_thread={single_thread}"
                );
            }
        }
    }
}

#[test]
fn settles_mixed_recordless_tiers_independently_of_file_order() {
    ccusage_test_support::install_pricing_snapshots();
    let change_last_tier = |mut contents: String, tier| {
        let last_count = contents.rfind('\n').unwrap() + 1;
        let settings = service_tier_line("2026-11-01T09:01:00.000Z", tier);
        contents.insert_str(last_count, &format!("{settings}\n"));
        contents
    };
    let named = recorded_requests(Some("priority"), &[Some("resp_x")]);
    let anonymous = change_last_tier(recorded_requests(Some("flex"), &[None, None]), "standard");
    let mixed = change_last_tier(
        recorded_requests(Some("priority"), &[Some("resp_x"), None]),
        "standard",
    );
    for files in [
        [&named, &anonymous, &mixed],
        [&named, &mixed, &anonymous],
        [&anonymous, &named, &mixed],
        [&anonymous, &mixed, &named],
        [&mixed, &named, &anonymous],
        [&mixed, &anonymous, &named],
    ] {
        let fixture = ccusage_test_support::Fixture::new();
        for (index, contents) in files.iter().enumerate() {
            fixture.write_file(format!("sessions/{index}.jsonl"), contents);
        }
        for single_thread in [true, false] {
            let shared = SharedArgs {
                single_thread,
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                ..SharedArgs::default()
            };
            let groups = load_groups_from_directory(
                &fixture.path("sessions"),
                &shared,
                AgentReportKind::Daily,
            )
            .unwrap();
            let usage = &groups["2026-11-01"].models["gpt-5.2"];
            assert_eq!(
                (
                    usage.usage.input_tokens,
                    usage.recorded_usage[CodexServiceTier::Fast].input_tokens,
                    usage.recorded_usage[CodexServiceTier::Standard].input_tokens,
                    usage.recorded_usage[CodexServiceTier::Flex].input_tokens,
                ),
                (2_000, 1_000, 1_000, 0),
                "single_thread={single_thread}"
            );
        }
    }
}

fn same_instant_rollout(id: &str, parent: Option<&str>, requests: u64, recorded: bool) -> String {
    let timestamp = "2026-11-01T09:15:00.000Z";
    let usage = json!({"input_tokens": 100, "cached_input_tokens": 0, "output_tokens": 1, "total_tokens": 101});
    let mut meta = json!({"id": id});
    if let Some(parent) = parent {
        meta["forked_from_id"] = parent.into();
        meta["source"] =
            json!({"subagent": {"thread_spawn": {"parent_thread_id": parent, "depth": 1}}});
    }
    let mut lines = vec![
        json!({"timestamp": "2026-11-01T09:00:00.000Z", "type": "session_meta", "payload": meta}).to_string(),
        json!({"timestamp": "2026-11-01T09:00:00.000Z", "type": "turn_context", "payload": {"model": "gpt-5.2"}}).to_string(),
    ];
    if parent.is_some() {
        lines.push(
            json!({"timestamp": "2026-11-01T09:10:00.000Z", "type": "event_msg", "payload": {"type": "thread_settings_applied", "thread_id": id, "thread_settings": {}}})
                .to_string(),
        );
    }
    for request in 1..=requests {
        if recorded {
            lines.push(
                json!({"timestamp": timestamp, "type": "token_usage_record", "payload": {"thread_id": id, "response_id": format!("resp_{id}_{request}"), "usage": usage}})
                    .to_string(),
            );
        }
        let total = json!({"input_tokens": 100 * request, "cached_input_tokens": 0, "output_tokens": request, "total_tokens": 101 * request});
        lines.push(
            json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": {"type": "token_count", "info": {"last_token_usage": usage, "total_token_usage": total}},
            })
            .to_string(),
        );
    }
    lines.join("\n")
}

fn assert_same_instant_requests(rollouts: &[(&str, String)], daily: u64, sessions: &[u64]) {
    ccusage_test_support::install_pricing_snapshots();
    let orders = [
        rollouts.iter().collect::<Vec<_>>(),
        rollouts.iter().rev().collect::<Vec<_>>(),
    ];
    for (order, files) in orders.iter().enumerate() {
        let fixture = ccusage_test_support::Fixture::new();
        for (position, (name, contents)) in files.iter().enumerate() {
            fixture.write_file(format!("sessions/{position}-{name}.jsonl"), contents);
        }
        let root = fixture.path("sessions");
        for single_thread in [true, false] {
            for bounded in [false, true] {
                let day = bounded.then(|| "20261101".parse().unwrap());
                let shared = SharedArgs {
                    single_thread,
                    since: day,
                    until: day,
                    timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                    ..SharedArgs::default()
                };
                let input = |kind| {
                    load_groups_from_directory(&root, &shared, kind)
                        .unwrap()
                        .into_iter()
                        .map(|(period, group)| (period, group.input_tokens))
                        .collect::<Vec<_>>()
                };
                let context =
                    format!("order={order}, single_thread={single_thread}, bounded={bounded}");
                assert_eq!(
                    input(AgentReportKind::Daily),
                    [("2026-11-01".to_string(), daily)],
                    "{context}"
                );
                let mut by_session = input(AgentReportKind::Session)
                    .into_iter()
                    .map(|(_, tokens)| tokens)
                    .collect::<Vec<_>>();
                by_session.sort_unstable();
                assert_eq!(by_session, sessions, "{context}");
            }
        }
    }
}

#[test]
fn keeps_a_spawned_children_own_request_beside_a_matching_recorded_parent_request() {
    assert_same_instant_requests(
        &[
            ("parent", same_instant_rollout("parent-xyz", None, 1, true)),
            (
                "child",
                same_instant_rollout("child-abc", Some("parent-xyz"), 1, false),
            ),
        ],
        200,
        &[100, 100],
    );
}

#[test]
fn settles_repeated_fork_requests_at_the_depth_used_for_pairing() {
    assert_same_instant_requests(
        &[
            ("parent", same_instant_rollout("parent-xyz", None, 1, true)),
            (
                "child",
                same_instant_rollout("child-abc", Some("parent-xyz"), 2, false),
            ),
            (
                "unrelated",
                same_instant_rollout("other-def", None, 2, false),
            ),
        ],
        400,
        &[100, 100, 200],
    );
}

#[test]
fn folds_many_sessions_to_the_same_groups_with_and_without_worker_threads() {
    ccusage_test_support::install_pricing_snapshots();
    let event_line = |session: usize, event: usize| {
        let day = (session * 7 + event) % 56;
        token_count_line(
            &format!(
                "2026-{:02}-{:02}T{:02}:{:02}:00.000Z",
                5 + day / 28,
                1 + day % 28,
                event % 24,
                session,
            ),
            if event.is_multiple_of(3) {
                "gpt-5"
            } else {
                "gpt-5-mini"
            },
            json!({
                "input_tokens": 100 + session * 10 + event,
                "output_tokens": 1 + event,
                "total_tokens": 101 + session * 10 + event * 2,
            }),
        )
    };
    let fixture = ccusage_test_support::Fixture::new();
    for session in 0..24 {
        let copied = (session % 2 == 1).then(|| (0..5).map(|event| event_line(session - 1, event)));
        let lines = copied
            .into_iter()
            .flatten()
            .chain((0..1_000).map(|event| event_line(session, event)))
            .collect::<Vec<_>>();
        fixture.write_file(
            format!("sessions/session-{session:02}.jsonl"),
            lines.join("\n"),
        );
    }
    let groups = |kind, single_thread| {
        let shared = SharedArgs {
            single_thread,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };
        load_groups_from_directory(&fixture.path("sessions"), &shared, kind)
            .unwrap()
            .into_iter()
            .map(|(period, group)| {
                let models = group
                    .models
                    .iter()
                    .map(|(model, usage)| (model.clone(), usage.total_tokens))
                    .collect::<Vec<_>>();
                (
                    period,
                    group.input_tokens,
                    group.output_tokens,
                    group.total_tokens,
                    group.last_activity,
                    models,
                )
            })
            .collect::<Vec<_>>()
    };

    for (kind, periods) in [
        (AgentReportKind::Daily, 56),
        (AgentReportKind::Monthly, 2),
        (AgentReportKind::Session, 24),
    ] {
        let single = groups(kind, true);
        assert_eq!(single.len(), periods, "{kind:?}");
        assert_eq!(groups(kind, false), single, "{kind:?}");
    }
}
