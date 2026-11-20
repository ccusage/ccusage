use std::assert_matches;
use std::ffi::OsString;

use ccusage_cli_parser::parse_from_with_config;
use ccusage_config::ConfigContext;
use ccusage_core::Color;
use ccusage_core::cli::{Command, PricingOverride};
use ccusage_test_support::{Environment, fs_fixture};

use super::block::{BlockAnchor, statusline_block_anchor, statusline_block_shared};
use super::cache::process_is_alive;
use super::context::statusline_context_color;
use super::*;

#[test]
fn transcript_context_uses_the_latest_usage_with_escaped_json_keys() {
    let fixture = ccusage_test_support::Fixture::new();
    let transcript = fixture.write_file(
        "transcript.jsonl",
        concat!(
            r#"{"type":"assistant","message":{"usage":{"input_tokens":10}}}"#, "\n",
            r#"{"t\u0079pe":"assist\u0061nt","message":{"usage":{"input\u005ftokens":20,"cache_read_input_tokens":3}}}"#, "\n",
            r#"{"type":"assistant","message":{"usage":{"input_tokens":"invalid"}}}"#,
        ),
    );

    let context =
        calculate_context_tokens_from_transcript(&transcript, None, true, &SharedArgs::default())
            .unwrap();

    assert_eq!(context.total_input_tokens, 23);
}

#[test]
fn a_cached_pid_is_alive_only_when_it_names_one_running_process() {
    assert!(process_is_alive(std::process::id()));
    for pid in [0, i32::MAX as u32 + 1, u32::MAX] {
        assert!(!process_is_alive(pid), "pid {pid}");
    }
}

#[test]
fn statusline_invocation_passes_config_pricing_overrides_to_cost_loading() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "ccusage.json": r#"{
                "defaults": {
                    "pricingOverrides": {
                        "third-party-model": {
                            "inputCostPerToken": 0.000001,
                            "outputCostPerToken": 0.000002
                        }
                    }
                },
                "commands": {
                    "statusline": {
                        "pricingOverrides": {
                            "third-party-model": {
                                "outputCostPerToken": 0.000003
                            }
                        }
                    }
                }
            }"#,
        "projects/project/session.jsonl": r#"{"timestamp":"2020-01-01T00:00:00.000Z","sessionId":"session","message":{"id":"message","model":"third-party-model","usage":{"input_tokens":100000,"output_tokens":100000}}}"#,
    });
    Environment::with_var("CLAUDE_CONFIG_DIR", fixture.root()).run(|| {
        let config_path = fixture.path("ccusage.json");
        let config_path = config_path.to_string_lossy().into_owned();
        let config_args = vec![
            "statusline".to_string(),
            "--config".to_string(),
            config_path.clone(),
        ];
        let config = ConfigContext::from_args(&config_args);
        let command = parse_from_with_config(
            [
                "ccusage",
                "statusline",
                "--config",
                config_path.as_str(),
                "--cost-source",
                "ccusage",
            ]
            .into_iter()
            .map(OsString::from),
            &config,
        )
        .unwrap();
        let Command::Statusline(args) = command else {
            panic!("expected statusline command");
        };

        let shared = statusline_shared(&args);
        let pricing = &shared.pricing_overrides["third-party-model"];
        assert_eq!(pricing.input_cost_per_token, Some(0.000001));
        assert_eq!(pricing.output_cost_per_token, Some(0.000003));

        let hook = StatuslineHook {
            session_id: "session".to_string(),
            transcript_path: fixture
                .path("transcript.jsonl")
                .to_string_lossy()
                .into_owned(),
            model: HookModel {
                id: Some("third-party-model".to_string()),
                display_name: "Third-party model".to_string(),
            },
            cost: None,
            context_window: Some(HookContext {
                total_input_tokens: 100_000,
                context_window_size: 200_000,
            }),
            effort: None,
        };
        let output = render_statusline(&hook, &args, &shared, &Diagnostics::new("claude")).unwrap();
        assert!(output.contains("💰 $0.40 session"), "{output}");
    })
}

#[test]
fn calculates_context_tokens_from_latest_assistant_transcript_line() {
    let fixture = fs_fixture!({
        "transcript.jsonl": [
            r#"{"type":"assistant","message":{"usage":{"input_tokens":1000,"output_tokens":999}}}"#,
            r#"not json"#,
            r#"{"type":"assistant","message":{"usage":{"input_tokens":2000,"cache_creation_input_tokens":100,"cache_read_input_tokens":50,"output_tokens":888}}}"#,
        ]
        .join("\n"),
    });

    let context = calculate_context_tokens_from_transcript(
        &fixture.path("transcript.jsonl"),
        None,
        true,
        &SharedArgs::default(),
    )
    .unwrap();

    assert_eq!(context.total_input_tokens, 2150);
    assert_eq!(context.context_window_size, 200_000);
}

#[test]
fn uses_model_context_limit_for_transcript_context_tokens() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "transcript.jsonl": r#"{"type":"assistant","message":{"usage":{"input_tokens":1000}}}"#,
    });

    let mut shared = SharedArgs::default();
    shared.pricing_overrides.insert(
        "test-model-context-limit".to_string(),
        PricingOverride {
            max_input_tokens: Some(1_500_000),
            ..Default::default()
        },
    );

    let context = calculate_context_tokens_from_transcript(
        &fixture.path("transcript.jsonl"),
        Some("test-model-context-limit"),
        true,
        &shared,
    )
    .unwrap();

    assert_eq!(context.total_input_tokens, 1000);
    assert_eq!(context.context_window_size, 1_500_000);
}

#[test]
fn pricing_92_rate_only_override_keeps_the_statusline_context_limit() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "transcript.jsonl": r#"{"type":"assistant","message":{"usage":{"input_tokens":1000}}}"#,
    });
    let model = "claude-opus-4-8";
    let mut shared = SharedArgs::default();
    let base = calculate_context_tokens_from_transcript(
        &fixture.path("transcript.jsonl"),
        Some(model),
        true,
        &shared,
    )
    .unwrap();
    shared.pricing_overrides.insert(
        model.into(),
        PricingOverride {
            input_cost_per_token: Some(50e-6),
            output_cost_per_token: Some(50e-6),
            ..Default::default()
        },
    );
    let overridden = calculate_context_tokens_from_transcript(
        &fixture.path("transcript.jsonl"),
        Some(model),
        true,
        &shared,
    )
    .unwrap();
    assert_eq!(base.context_window_size, 1_000_000);
    assert_eq!(overridden.context_window_size, base.context_window_size);
}

#[test]
fn colors_statusline_context_percentage_by_threshold() {
    let shared = SharedArgs {
        color: ccusage_core::cli::ColorChoice::Always,
        ..SharedArgs::default()
    };
    let args = StatuslineArgs::default();

    assert_matches!(statusline_context_color(60, &args), Color::Yellow);
    assert!(format_statusline_context(120_000, 200_000, &args, &shared).contains("60%"));
}

#[test]
fn builds_statusline_today_filter_from_timezone() {
    let args = StatuslineArgs {
        timezone: Some("Asia/Tokyo".parse().unwrap()),
        ..StatuslineArgs::default()
    };
    let mut shared = SharedArgs {
        offline: true,
        ..SharedArgs::default()
    };
    shared.pricing_overrides.insert(
        "statusline-model".to_string(),
        PricingOverride {
            input_cost_per_token: Some(1e-6),
            ..Default::default()
        },
    );
    let now = TimestampMs::from_millis(1_779_380_820_000);

    let today_shared = statusline_today_shared(&args, &shared, now);

    assert_eq!(
        today_shared.since.map(|date| date.to_string()).as_deref(),
        Some("20260522")
    );
    assert_eq!(
        today_shared.until.map(|date| date.to_string()).as_deref(),
        Some("20260522")
    );
    assert_eq!(
        today_shared
            .timezone
            .as_ref()
            .map(|timezone| timezone.name()),
        Some("Asia/Tokyo")
    );
    assert!(
        today_shared
            .pricing_overrides
            .contains_key("statusline-model")
    );
}

#[test]
fn bounds_statusline_block_load_behind_the_session_window() {
    let args = StatuslineArgs {
        timezone: Some("Asia/Tokyo".parse().unwrap()),
        ..StatuslineArgs::default()
    };
    let shared = SharedArgs {
        offline: true,
        ..SharedArgs::default()
    };
    let now = TimestampMs::from_millis(1_779_380_820_000);

    let block_shared = statusline_block_shared(&args, &shared, now, 1);

    assert_eq!(
        block_shared.since.map(|date| date.to_string()).as_deref(),
        Some("20260520")
    );
    assert_eq!(
        statusline_block_shared(&args, &shared, now, 7)
            .since
            .map(|date| date.to_string())
            .as_deref(),
        Some("20260514")
    );
    assert_eq!(block_shared.until, None);
    assert_eq!(
        block_shared
            .timezone
            .as_ref()
            .map(|timezone| timezone.name()),
        Some("Asia/Tokyo")
    );
    assert!(block_shared.offline);
}

#[test]
fn anchors_statusline_blocks_after_the_latest_long_pause() {
    const HOUR: i64 = 60 * 60 * 1000;
    let now = TimestampMs::from_millis(1_779_380_820_000);
    let at = |hours_ago: i64| TimestampMs::from_millis(now.as_millis() - hours_ago * HOUR);
    let session = 5 * HOUR;

    assert_eq!(
        statusline_block_anchor(&[], now, session),
        BlockAnchor::NoActiveBlock
    );
    assert_eq!(
        statusline_block_anchor(&[at(30), at(6)], now, session),
        BlockAnchor::NoActiveBlock
    );
    assert_eq!(
        statusline_block_anchor(&[at(40), at(30), at(20), at(18), at(1)], now, session),
        BlockAnchor::From(at(1))
    );
    assert_eq!(
        statusline_block_anchor(
            &[at(30), at(20), at(16), at(12), at(8), at(4)],
            now,
            session
        ),
        BlockAnchor::From(at(20))
    );
    assert_eq!(
        statusline_block_anchor(&[at(9), at(4), at(1)], now, session),
        BlockAnchor::NeedsMoreHistory
    );
    assert_eq!(
        statusline_block_anchor(&[at(1), at(-8)], now, session),
        BlockAnchor::NeedsFullHistory
    );
}

#[test]
fn reuses_statusline_cache_while_fresh_and_transcript_unchanged() {
    let cache = StatuslineCache {
        date: "2026-01-01T00:00:00.000Z".to_string(),
        last_output: "cached status".to_string(),
        last_update_time: 10_000,
        transcript_path: "/tmp/transcript.jsonl".to_string(),
        transcript_mtime: 123,
        is_updating: false,
        pid: None,
    };

    assert_eq!(
        cached_statusline_output(&cache, 123, 10_500, 1),
        Some("cached status")
    );
}

#[test]
fn invalidates_statusline_cache_when_transcript_changes() {
    let cache = StatuslineCache {
        date: "2026-01-01T00:00:00.000Z".to_string(),
        last_output: "cached status".to_string(),
        last_update_time: 10_000,
        transcript_path: "/tmp/transcript.jsonl".to_string(),
        transcript_mtime: 123,
        is_updating: false,
        pid: None,
    };

    assert_eq!(cached_statusline_output(&cache, 456, 10_500, 1), None);
}

#[test]
fn returns_stale_statusline_cache_while_live_process_is_updating() {
    let cache = StatuslineCache {
        date: "2026-01-01T00:00:00.000Z".to_string(),
        last_output: "stale status".to_string(),
        last_update_time: 10_000,
        transcript_path: "/tmp/transcript.jsonl".to_string(),
        transcript_mtime: 123,
        is_updating: true,
        pid: Some(std::process::id()),
    };

    assert_eq!(
        cached_statusline_output(&cache, 456, 20_000, 1),
        Some("stale status")
    );
}

#[test]
fn resolves_model_label_from_alias_map() {
    let mut aliases = std::collections::HashMap::new();
    aliases.insert(
        "arn:aws:bedrock:ap-northeast-1:012345678910:application-inference-profile/abcde12345"
            .to_string(),
        "claude-opus-4-6".to_string(),
    );

    assert_eq!(
        resolve_model_label(
            &aliases,
            "arn:aws:bedrock:ap-northeast-1:012345678910:application-inference-profile/abcde12345"
        ),
        "claude-opus-4-6"
    );
}

#[test]
fn falls_back_to_display_name_when_no_alias_matches() {
    let aliases = std::collections::HashMap::new();

    assert_eq!(resolve_model_label(&aliases, "Opus 4.1"), "Opus 4.1");
}

#[test]
fn appends_effort_level_to_model_segment() {
    let effort = HookEffort {
        level: "high".to_string(),
    };

    assert_eq!(
        format_model_segment("Fable 5", Some(&effort)),
        "Fable 5 (high)"
    );
}

#[test]
fn omits_effort_from_model_segment_when_absent_or_empty() {
    let empty = HookEffort {
        level: String::new(),
    };

    assert_eq!(format_model_segment("Fable 5", None), "Fable 5");
    assert_eq!(format_model_segment("Fable 5", Some(&empty)), "Fable 5");
}

#[test]
fn parses_statusline_hook_with_and_without_effort() {
    let with_effort: StatuslineHook = serde_json::from_str(
        r#"{
                "session_id": "session",
                "transcript_path": "/tmp/transcript.jsonl",
                "model": {"id": "claude-fable-5", "display_name": "Fable 5"},
                "effort": {"level": "xhigh"}
            }"#,
    )
    .unwrap();
    let without_effort: StatuslineHook = serde_json::from_str(
        r#"{
                "session_id": "session",
                "transcript_path": "/tmp/transcript.jsonl",
                "model": {"id": "claude-sonnet-4-20250514", "display_name": "Sonnet 4"}
            }"#,
    )
    .unwrap();

    assert_eq!(
        with_effort.effort.map(|effort| effort.level).as_deref(),
        Some("xhigh")
    );
    assert!(without_effort.effort.is_none());
}
