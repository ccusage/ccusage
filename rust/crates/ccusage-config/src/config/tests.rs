#[test]
fn malformed_discovered_files_warn_and_continue_discovery() {
    for raw in ["", "[]", "null", "{", "{\"defaults\":{},}", "// comment"] {
        let fixture = fs_fixture!({
            "first.json": raw,
            "second.json": r#"{"defaults":{"offline":true,"json":true}}"#,
        });
        let (file, error, warnings) = load_config_paths(
            false,
            vec![fixture.path("first.json"), fixture.path("second.json")],
        );
        assert!(error.is_none(), "{raw}: {error:?}");
        let file = file.unwrap();
        assert_eq!(file.path, fixture.path("second.json"));
        assert_eq!(file.layers["defaults"].shared.offline, Some(true));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("first.json"));
        let (file, error, _) = load_config_paths(true, vec![fixture.path("first.json")]);
        assert!(file.is_none());
        assert!(error.is_some());
    }
}

#[test]
fn keeps_named_pi_store_errors_gated_to_commands_that_read_them() {
    for raw in [
        r#"{"pi":{"stores":[{"name":"codex","path":"/tmp/store"}]}}"#,
        r#"{"pi":{"stores":[{"name":"custom"}]}}"#,
    ] {
        let fixture = fs_fixture!({"ccusage.json": raw});
        ccusage_test_support::Environment::with_var("CLAUDE_CONFIG_DIR", fixture.root()).run(|| {
            for command in [
                vec!["statusline"],
                vec!["claude", "blocks"],
                vec!["claude", "session"],
                vec!["codex", "daily"],
                vec!["pi", "daily"],
                vec!["pi", "session"],
                vec!["pi:daily"],
            ] {
                let args = command
                    .iter()
                    .map(|arg| arg.to_string())
                    .collect::<Vec<_>>();
                assert!(
                    ConfigContext::from_args(&args).config_error().is_none(),
                    "{command:?}"
                );
            }
            for command in [
                vec!["daily"],
                vec!["weekly"],
                vec!["monthly"],
                vec!["session"],
            ] {
                let args = command
                    .iter()
                    .map(|arg| arg.to_string())
                    .collect::<Vec<_>>();
                assert!(
                    ConfigContext::from_args(&args).config_error().is_some(),
                    "{command:?}"
                );
            }
        })
    }
}

#[test]
fn discovered_date_errors_only_affect_commands_reading_the_layer() {
    let fixture = ccusage_test_support::fs_fixture!({
        "ccusage.json": r#"{"defaults":{"mode":"a.since:","json":true},"commands":{"weekly":{"since":"nope"}}}"#,
    });
    ccusage_test_support::Environment::with_var("CLAUDE_CONFIG_DIR", fixture.root()).run(|| {
        for command in [
            vec!["daily"],
            vec!["monthly"],
            vec!["session"],
            vec!["claude", "session"],
            vec!["codex", "daily"],
            vec!["statusline"],
        ] {
            let args = command
                .iter()
                .map(|arg| arg.to_string())
                .collect::<Vec<_>>();
            let context = ConfigContext::from_args(&args);
            assert!(
                context.config_error().is_none(),
                "{command:?}: {:?}",
                context.config_error()
            );
            let mut shared = SharedArgs::default();
            context.apply_shared(&mut shared);
            assert!(shared.json);
        }
        for command in [vec!["weekly"], vec!["codex", "weekly"]] {
            let args = command
                .iter()
                .map(|arg| arg.to_string())
                .collect::<Vec<_>>();
            let context = ConfigContext::from_args(&args);
            assert!(
                context
                    .config_error()
                    .unwrap()
                    .contains("commands.weekly.since")
            );
        }
    })
}

#[test]
fn invalid_discovered_block_limits_drop_only_the_field() {
    for (field, value) in [
        ("tokenLimit", json!(0)),
        ("tokenLimit", json!("0")),
        ("tokenLimit", json!("abc")),
        ("sessionLength", json!(0)),
        ("sessionLength", json!(-1)),
        ("sessionLength", json!(1e300)),
        ("sessionLength", json!("NaN")),
    ] {
        let raw = json!({"commands": {"blocks": {field: value, "active": true, "json": true}}});
        let fixture = fs_fixture!({"selected.json": raw.to_string()});
        let (file, error, warnings) = load_config_paths(false, vec![fixture.path("selected.json")]);
        assert!(error.is_none(), "{error:?}");
        let file = file.unwrap();
        let options = &file.layers["commands.blocks"];
        assert_eq!(options.blocks.active, Some(true));
        assert_eq!(options.shared.json, Some(true));
        assert!(options.blocks.token_limit.is_none());
        assert!(options.blocks.session_length.is_none());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains(&format!("commands.blocks.{field}")),
            "{warnings:?}"
        );
        assert!(warnings[0].contains("selected.json"));
    }
}

#[test]
fn invalid_explicit_block_limits_are_errors() {
    for (field, value) in [
        ("tokenLimit", json!(0)),
        ("tokenLimit", json!("abc")),
        ("sessionLength", json!(0)),
        ("sessionLength", json!(-1)),
        ("sessionLength", json!(1e300)),
    ] {
        let raw = json!({"commands": {"blocks": {field: value, "json": true}}});
        let fixture = fs_fixture!({"selected.json": raw.to_string()});
        let (file, error, _) = load_config_paths(true, vec![fixture.path("selected.json")]);
        assert!(file.is_none());
        assert!(error.unwrap().contains(&format!("commands.blocks.{field}")));
    }
}

#[test]
fn context_thresholds_validate_the_merged_layers() {
    let raw = json!({
        "defaults": {"contextLowThreshold": 90},
        "commands": {"statusline": {"contextMediumThreshold": 95}}
    });
    let fixture = fs_fixture!({"selected.json": raw.to_string()});
    let config = ConfigContext::from_args(&[
        "statusline".into(),
        "--config".into(),
        fixture.path("selected.json").display().to_string(),
    ]);
    assert!(
        config.config_error().is_none(),
        "{:?}",
        config.config_error()
    );
    let mut args = StatuslineArgs::default();
    config.apply_statusline_args(&mut args);
    assert_eq!(
        config.context_thresholds(None, None).unwrap(),
        ContextThresholds::new(90, 95).unwrap()
    );
}

#[test]
fn discovered_context_thresholds_drop_the_invalid_override() {
    let mut config = context(
        json!({
            "defaults": {"contextLowThreshold": 70},
            "commands": {"statusline": {"contextMediumThreshold": 60, "cache": false}}
        }),
        "statusline",
        None,
        "statusline",
    );
    config.config_origin = Some((PathBuf::from("selected.json"), false));
    assert!(config.config_error().is_none());
    let mut args = StatuslineArgs::default();
    config.apply_statusline_args(&mut args);
    assert_eq!(
        config.context_thresholds(None, None).unwrap(),
        ContextThresholds::new(70, 80).unwrap()
    );
    assert!(!args.cache);
    assert_eq!(config.warnings().len(), 1);
    assert!(
        config.warnings()[0].contains("selected.json: commands.statusline.contextMediumThreshold")
    );
}

#[test]
fn discovered_threshold_does_not_invalidate_a_cli_flag() {
    let mut config = context(
        json!({"defaults": {"contextMediumThreshold": 40}}),
        "statusline",
        None,
        "statusline",
    );
    config.config_origin = Some((PathBuf::from("selected.json"), false));
    assert_eq!(
        config.context_thresholds(Some(60), None).unwrap(),
        ContextThresholds::new(60, 80).unwrap()
    );
    assert_eq!(config.warnings().len(), 1);
    assert!(config.context_thresholds(Some(60), Some(40)).is_err());
}

#[test]
fn numeric_zero_token_limit_reports_the_positive_limit_requirement() {
    let fixture = fs_fixture!({
        "selected.json": r#"{"commands":{"blocks":{"tokenLimit":0}}}"#
    });
    let (_, _, warnings) = load_config_paths(false, vec![fixture.path("selected.json")]);
    assert!(warnings[0].contains("Expected max or a positive whole number"));
}

#[test]
fn explicit_unordered_context_thresholds_are_errors() {
    let fixture = fs_fixture!({"selected.json": r#"{"commands":{"statusline":{"contextLowThreshold":80,"cache":false}}}"#});
    let config = ConfigContext::from_args(&[
        "statusline".into(),
        "--config".into(),
        fixture.path("selected.json").display().to_string(),
    ]);
    assert!(
        config
            .context_thresholds(None, None)
            .unwrap_err()
            .contains("commands.statusline.contextLowThreshold")
    );
}

#[test]
fn unreadable_discovered_config_does_not_select_another_file() {
    let fixture = fs_fixture!({"lower.json": r#"{"defaults":{"json":true}}"#});
    let selected = fixture.create_dir_all("selected.json");
    let (file, error, _) = load_config_paths(false, vec![selected, fixture.path("lower.json")]);
    assert!(file.is_none());
    assert!(error.unwrap().contains("selected.json"));
}
#[test]
fn token_limits_accept_numbers_and_strings() {
    for raw in [
        r#"{"commands":{"blocks":{"tokenLimit":500000}}}"#,
        r#"{"commands":{"blocks":{"tokenLimit":"500000"}}}"#,
    ] {
        let fixture = fs_fixture!({"config.json": raw});
        let config = ConfigContext::from_args(&[
            "blocks".into(),
            "--config".into(),
            fixture.path("config.json").display().to_string(),
        ]);
        assert!(
            config.config_error().is_none(),
            "{:?}",
            config.config_error()
        );
        let mut args = BlocksArgs {
            shared: SharedArgs::default(),
            active: false,
            recent: false,
            token_limit: None,
            session_length: ccusage_cli::SessionLength::DEFAULT,
        };
        config.apply_blocks_args(&mut args);
        assert_eq!(args.token_limit, Some("500000".parse().unwrap()));
    }
}
#[test]
fn discovered_config_retains_valid_fields_without_falling_through() {
    let fixture = fs_fixture!({
        "selected.json": r#"{"defaults":{"json":true,"timezone":"Asia/Tokyo","mode":"Calculate","pricingOverrides":{"sample":{"inputCostPerToken":0.1,"outputCostPerToken":"bad"}}},"commands":{"weekly":{"startOfWeek":"Sunday"}}}"#,
        "lower.json": r#"{"defaults":{"json":false,"timezone":"UTC"}}"#,
    });
    let (file, error, warnings) = load_config_paths(
        false,
        vec![fixture.path("selected.json"), fixture.path("lower.json")],
    );
    assert!(error.is_none(), "{error:?}");
    let file = file.unwrap();
    assert_eq!(file.layers["defaults"].shared.json, Some(true));
    assert_eq!(
        file.layers["defaults"].shared.timezone.as_deref(),
        Some("Asia/Tokyo")
    );
    assert_eq!(
        file.layers["defaults"]
            .shared
            .pricing_overrides
            .as_ref()
            .unwrap()["sample"]
            .input_cost_per_token,
        Some(0.1)
    );
    assert_eq!(warnings.len(), 3, "{warnings:?}");
    for path in [
        "defaults.mode",
        "defaults.pricingOverrides.sample.outputCostPerToken",
        "commands.weekly.startOfWeek",
    ] {
        assert!(
            warnings.iter().any(|warning| warning.contains(path)),
            "{warnings:?}"
        );
    }
}

#[test]
fn discovered_invalid_pi_stores_keep_the_selected_file() {
    for raw in [
        r#"{"pi":{"stores":[{"name":"claude","path":"/tmp/store"}]}}"#,
        r#"{"pi":{"stores":[{"name":"custom"}]}}"#,
    ] {
        let fixture = fs_fixture!({"selected.json": raw});
        let (file, error, _) = load_config_paths(false, vec![fixture.path("selected.json")]);
        assert!(file.unwrap().pi_stores_error.is_some(), "{raw}");
        assert!(error.is_none(), "{raw}");
    }
}

#[test]
fn discovered_default_date_errors_are_fatal_except_for_statusline() {
    for field in ["since", "until"] {
        let fixture =
            fs_fixture!({"ccusage.json": json!({"defaults": {field: "nope"}}).to_string()});
        ccusage_test_support::Environment::with_var("CLAUDE_CONFIG_DIR", fixture.root()).run(|| {
            assert!(
                ConfigContext::from_args(&["daily".into()])
                    .config_error()
                    .is_some()
            );
            assert!(
                ConfigContext::from_args(&["statusline".into()])
                    .config_error()
                    .is_none()
            );
        })
    }
}

#[test]
fn unknown_pi_store_fields_warn_without_discarding_stores() {
    let value = json!({
        "pi": {
            "stores": [
                { "name": "work", "path": "/tmp/work", "alias": "w" },
                { "name": "side", "path": "/tmp/side", "enabled": true }
            ]
        }
    });
    let expected = [
        "Unknown ccusage config key: pi.stores[0].alias",
        "Unknown ccusage config key: pi.stores[1].enabled",
    ];
    let strict = crate::layers::ConfigFile::parse(value.clone()).unwrap();
    let discovered = crate::layers::ConfigFile::parse_discovered(value.clone()).unwrap();

    assert_eq!(strict.warnings, expected);
    assert_eq!(discovered.warnings, expected);
    assert_eq!(parse_named_pi_stores(&value).unwrap().len(), 2);
}

#[test]
fn unknown_keys_warn_without_discarding_valid_options() {
    let config = config_context_from_json(
        r#"{"defaults":{"json":true,"jsoon":true,"pricingOverrides":{"model":{"inputCostPerToken":0.1,"inputPrice":9}}},"commands":{"blocks":{"live":true}},"cluade":{}}"#,
    );
    assert!(config.config_error().is_none());
    let mut shared = SharedArgs::default();
    config.apply_shared(&mut shared);
    assert!(shared.json);
    assert_eq!(
        shared.pricing_overrides["model"].input_cost_per_token,
        Some(0.1)
    );
    for path in [
        "defaults.jsoon",
        "defaults.pricingOverrides.model.inputPrice",
        "commands.blocks.live",
        "cluade",
    ] {
        assert!(
            config
                .warnings()
                .iter()
                .any(|warning| warning.ends_with(path)),
            "{path}: {:?}",
            config.warnings
        );
    }
    assert_eq!(config.warnings().len(), 4);
}

#[test]
fn explicit_config_rejects_wrong_types_and_bad_enums_with_paths() {
    for (raw, path) in [
        (r#"{"defaults":{"json":"yes"}}"#, "defaults.json"),
        (
            r#"{"commands":{"daily":{"mode":"invalid"}}}"#,
            "commands.daily.mode",
        ),
        (
            r#"{"commands":{"statusline":{"contextLowThreshold":256}}}"#,
            "commands.statusline.contextLowThreshold",
        ),
        (
            r#"{"defaults":{"pricingOverrides":{"model":{"inputCostPerToken":"cheap"}}}}"#,
            "defaults.pricingOverrides.model.inputCostPerToken",
        ),
    ] {
        let config = config_context_from_json(raw);
        let error = config
            .config_error()
            .expect("invalid explicit file must fail");
        assert!(error.contains(path), "{error}");
    }
}

#[test]
fn explicit_config_rejects_broken_and_missing_files() {
    let fixture = fs_fixture!({ "broken.json": "{" });
    for name in ["broken.json", "missing.json"] {
        let path = fixture.path(name);
        let config = ConfigContext::from_args(&[
            "daily".into(),
            "--config".into(),
            path.display().to_string(),
        ]);
        let error = config
            .config_error()
            .expect("invalid explicit path must fail");
        assert!(error.contains(name), "{error}");
    }
}

#[test]
fn colon_commands_select_only_registered_reports() {
    for (name, agent) in [
        ("codex:daily", Some("codex")),
        ("claude:statusline", Some("claude")),
        ("codex:weekly", None),
        ("unknown:daily", None),
    ] {
        let command = super::detect_config_command(&[name.to_string()]);
        assert_eq!(command.agent.as_deref(), agent, "{name}");
    }
}

#[test]
fn legacy_claude_commands_select_claude_layers() {
    for (args, expected) in [
        (&["blocks"][..], Some("claude")),
        (&["session", "--id", "target"], Some("claude")),
        (&["session", "--id=target"], Some("claude")),
        (&["session", "-i", "target"], Some("claude")),
        (&["session"], None),
        (&["session", "--jq", "--id"], None),
    ] {
        let args = args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
        let command = super::detect_config_command(&args);
        assert_eq!(command.agent.as_deref(), expected, "{args:?}");
    }
}

use serde_json::{Value, json};

use super::{
    apply::{
        apply_config_to_blocks_args, apply_config_to_shared, apply_config_to_statusline_args,
        apply_config_to_weekly_args,
    },
    discovery::load_config_paths,
    pi_stores::{
        parse_named_pi_stores, reserved_named_pi_store_names, validate_named_pi_store_name,
    },
    pricing::merge_pricing_overrides,
    *,
};
use crate::config_schema::{ConfigPricingOverride, NAMED_PI_STORE_NAME_PATTERN};
use ccusage_cli::{
    BlocksArgs, CliConfig, CodexSpeed, ColorChoice, CostMode, SharedArgs, SortOrder,
    StatuslineArgs, VisualBurnRate, WeekDay, WeeklyArgs,
};
use ccusage_test_support::{Environment, fs_fixture};
use std::collections::BTreeSet;

#[test]
fn applies_schema_backed_shared_options() {
    let config = context(
        json!({
            "defaults": {
                "since": "2026-01-01",
                "until": "2026-01-31",
                "json": true,
                "mode": "calculate",
                "debug": true,
                "debugSamples": 9,
                "order": "desc",
                "breakdown": true,
                "offline": true,
                "noOffline": true,
                "color": true,
                "noColor": true,
                "timezone": "Asia/Tokyo",
                "jq": ".totals",
                "compact": true,
                "singleThread": true,
                "noCost": true,
            }
        }),
        "daily",
        None,
        "daily",
    );
    let mut shared = SharedArgs::default();

    apply_config_to_shared(&mut shared, &config);

    assert_eq!(
        shared.since.map(|date| date.to_string()).as_deref(),
        Some("20260101")
    );
    assert_eq!(
        shared.until.map(|date| date.to_string()).as_deref(),
        Some("20260131")
    );
    assert!(shared.json);
    assert_eq!(shared.mode, CostMode::Calculate);
    assert!(shared.debug);
    assert_eq!(shared.debug_samples, 9);
    assert_eq!(shared.order, Some(SortOrder::Desc));
    assert!(shared.breakdown);
    assert!(!shared.offline);
    assert_eq!(shared.color, ColorChoice::Never);
    assert_eq!(config.timezone(), Some("Asia/Tokyo"));
    assert_eq!(shared.jq.as_deref(), Some(".totals"));
    assert!(shared.compact);
    assert!(shared.single_thread);
    assert!(shared.no_cost);
}

#[test]
fn applies_schema_backed_report_specific_options() {
    let config = context(
        json!({
            "commands": {
                "blocks": {
                    "active": true,
                    "recent": true,
                    "tokenLimit": "500000",
                    "sessionLength": 6.5
                },
                "statusline": {
                    "offline": false,
                    "noOffline": true,
                    "visualBurnRate": "emoji-text",
                    "costSource": "both",
                    "cache": false,
                    "noCache": true,
                    "refreshInterval": 3,
                    "contextLowThreshold": 45,
                    "contextMediumThreshold": 75,
                    "timezone": "Asia/Tokyo",
                    "debug": true
                }
            }
        }),
        "blocks",
        None,
        "blocks",
    );
    let mut blocks = BlocksArgs {
        shared: SharedArgs::default(),
        active: false,
        recent: false,
        token_limit: None,
        session_length: ccusage_cli::SessionLength::DEFAULT,
    };
    apply_config_to_blocks_args(&mut blocks, &config);

    assert!(blocks.active);
    assert!(blocks.recent);
    assert_eq!(blocks.token_limit, Some("500000".parse().unwrap()));
    assert_eq!(blocks.session_length.as_millis(), 23_400_000);

    let mut config = context(
        json!({
            "commands": {
                "statusline": {
                    "offline": false,
                    "noOffline": true,
                    "visualBurnRate": "emoji-text",
                    "costSource": "both",
                    "cache": false,
                    "noCache": true,
                    "refreshInterval": 3,
                    "contextLowThreshold": 45,
                    "contextMediumThreshold": 75,
                    "timezone": "Asia/Tokyo",
                    "debug": true
                }
            }
        }),
        "statusline",
        None,
        "statusline",
    );
    config.config_origin = Some((PathBuf::from("config.json"), true));
    let mut statusline = StatuslineArgs::default();
    apply_config_to_statusline_args(&mut statusline, &config);

    assert!(!statusline.offline);
    assert_eq!(statusline.visual_burn_rate, VisualBurnRate::EmojiText);
    assert_eq!(statusline.cost_source, ccusage_cli::CostSource::Both);
    assert!(!statusline.cache);
    assert_eq!(statusline.refresh_interval, 3);
    assert_eq!(config.context_thresholds(None, None).unwrap().low(), 45);
    assert_eq!(config.context_thresholds(None, None).unwrap().medium(), 75);
    assert_eq!(config.timezone(), Some("Asia/Tokyo"));
    assert!(statusline.debug);
}

#[test]
fn applies_schema_backed_agent_specific_options() {
    let mut weekly = WeeklyArgs {
        shared: SharedArgs::default(),
        start_of_week: WeekDay::Sunday,
    };
    apply_config_to_weekly_args(
        &mut weekly,
        &context(
            json!({
                "claude": {
                    "commands": {
                        "weekly": {
                            "startOfWeek": "monday"
                        }
                    }
                }
            }),
            "claude weekly",
            Some("claude"),
            "weekly",
        ),
    );

    assert_eq!(weekly.start_of_week, WeekDay::Monday);

    let codex = context(
        json!({"codex":{"defaults":{"speed":"fast"}}}),
        "codex daily",
        Some("codex"),
        "daily",
    );
    assert_eq!(codex.agent_options("codex").codex_speed, CodexSpeed::Fast);
    let pi = context(
        json!({"pi":{"defaults":{"piPath":"/tmp/pi-sessions","piAgentDir":"/tmp/pi-agent"}}}),
        "pi daily",
        Some("pi"),
        "daily",
    );
    assert_eq!(
        pi.agent_options("pi").pi_path.as_deref(),
        Some("/tmp/pi-sessions")
    );
    assert_eq!(
        pi.agent_options("pi").pi_agent_dir.as_deref(),
        Some("/tmp/pi-agent")
    );
    let openclaw = context(
        json!({"openclaw":{"defaults":{"openClawPath":"/tmp/openclaw"}}}),
        "openclaw daily",
        Some("openclaw"),
        "daily",
    );
    assert_eq!(
        openclaw.agent_options("openclaw").open_claw_path.as_deref(),
        Some("/tmp/openclaw")
    );
}

#[test]
fn grok_namespace_keeps_shared_report_options() {
    let config = context(
        json!({
            "grok": {
                "defaults": { "offline": true },
                "commands": { "session": { "json": true } }
            }
        }),
        "grok session",
        Some("grok"),
        "session",
    );
    let mut shared = SharedArgs::default();

    apply_config_to_shared(&mut shared, &config);

    assert!(shared.offline);
    assert!(shared.json);
}

#[test]
fn zcode_namespace_keeps_shared_report_options() {
    let config = context(
        json!({
            "zcode": {
                "defaults": { "offline": true },
                "commands": { "session": { "json": true } }
            }
        }),
        "zcode session",
        Some("zcode"),
        "session",
    );
    let mut shared = SharedArgs::default();

    apply_config_to_shared(&mut shared, &config);

    assert!(shared.offline);
    assert!(shared.json);
}

#[test]
fn agent_command_sections_override_the_generic_report_section() {
    let generic = json!({ "mode": "display", "since": "20260101" });
    let cases = [
        (
            "space form",
            json!({ "daily": generic, "codex daily": { "mode": "calculate" } }),
            CostMode::Calculate,
        ),
        (
            "colon form",
            json!({ "daily": generic, "codex:daily": { "mode": "calculate" } }),
            CostMode::Calculate,
        ),
        (
            "colon form over space form",
            json!({
                "daily": generic,
                "codex daily": { "mode": "calculate" },
                "codex:daily": { "mode": "auto" }
            }),
            CostMode::Auto,
        ),
    ];
    for (name, commands, mode) in cases {
        let value = json!({ "commands": commands });
        let focused = context(value.clone(), "codex daily", Some("codex"), "daily");
        let source = context(value, "daily", None, "daily").for_dependency("codex");

        for config in [focused, source] {
            let mut shared = SharedArgs::default();
            config.apply_shared(&mut shared);
            assert_eq!(shared.mode, mode, "{name}");
            assert_eq!(
                shared.since.map(|date| date.to_string()).as_deref(),
                Some("20260101"),
                "{name}"
            );
        }
    }
}

#[test]
fn space_form_section_overrides_the_generic_report_section_for_every_agent_command_shape() {
    let raw = r#"{
            "commands": {
                "daily": { "mode": "display", "since": "20260101" },
                "codex daily": { "mode": "calculate" }
            }
        }"#;
    for command in [&["codex", "daily"][..], &["codex:daily"], &["codex"]] {
        let config = config_context_for_command(raw, command);
        let mut shared = SharedArgs::default();
        config.apply_shared(&mut shared);
        assert_eq!(shared.mode, CostMode::Calculate, "{command:?}");
        assert_eq!(
            shared.since.map(|date| date.to_string()).as_deref(),
            Some("20260101"),
            "{command:?}"
        );
    }
}

#[test]
fn source_view_reads_the_sections_a_focused_command_would() {
    let config = context(
        json!({
            "defaults": {
                "mode": "display",
                "since": "20260101"
            },
            "commands": {
                "daily": {
                    "breakdown": true
                },
                "codex:daily": {
                    "timezone": "UTC"
                }
            },
            "codex": {
                "defaults": {
                    "speed": "fast",
                    "since": "20260301"
                },
                "commands": {
                    "daily": {
                        "mode": "calculate"
                    }
                }
            }
        }),
        "daily",
        None,
        "daily",
    );

    let codex = config.for_dependency("codex");
    let mut shared = SharedArgs::with_defaults();
    codex.apply_shared(&mut shared);
    let speed = codex.agent_options("codex").codex_speed;

    assert_eq!(
        shared.since.map(|date| date.to_string()).as_deref(),
        Some("20260301")
    );
    assert_eq!(shared.mode, CostMode::Calculate);
    assert_eq!(codex.timezone(), Some("UTC"));
    assert!(shared.breakdown);
    assert_eq!(speed, CodexSpeed::Fast);
}

#[test]
fn cline_weekly_config_matches_focused_and_unified_source_views() {
    let value = json!({
        "defaults": {
            "mode": "auto",
            "since": "20260101",
            "pricingOverrides": {
                "cline-model": {
                    "inputCostPerToken": 0.000001
                }
            }
        },
        "commands": {
            "weekly": {
                "mode": "display"
            },
            "cline:weekly": {
                "pricingOverrides": {
                    "cline-model": {
                        "inputCostPerToken": 0.000002
                    }
                }
            }
        },
        "cline": {
            "defaults": {
                "mode": "display",
                "offline": true,
                "since": "20260301",
                "pricingOverrides": {
                    "cline-model": {
                        "outputCostPerToken": 0.00002
                    }
                }
            },
            "commands": {
                "weekly": {
                    "mode": "calculate",
                    "json": true,
                    "timezone": "UTC",
                    "pricingOverrides": {
                        "cline-model": {
                            "inputCostPerToken": 0.000003
                        }
                    }
                }
            }
        }
    });
    let unified = context(value.clone(), "weekly", None, "weekly");
    let focused = context(value, "cline weekly", Some("cline"), "weekly");
    let unified_source = unified.for_dependency("cline");

    for config in [&focused, &unified_source] {
        let mut shared = SharedArgs::default();
        apply_config_to_shared(&mut shared, config);

        assert_eq!(shared.mode, CostMode::Calculate);
        assert_eq!(
            shared.since.map(|date| date.to_string()).as_deref(),
            Some("20260301")
        );
        assert!(shared.offline);
        assert!(shared.json);
        assert_eq!(config.timezone(), Some("UTC"));
        assert_eq!(
            shared.pricing_overrides["cline-model"].input_cost_per_token,
            Some(0.000003)
        );
        assert_eq!(
            shared.pricing_overrides["cline-model"].output_cost_per_token,
            Some(0.00002)
        );
    }
}

#[test]
fn dependency_views_resolve_effective_options_without_raw_json() {
    let config = context(
        json!({
            "defaults": {
                "mode": "display"
            },
            "commands": {
                "pi:daily": {
                    "json": true
                }
            },
            "openclaw": {
                "defaults": {
                    "offline": true
                }
            },
            "cline": {
                "commands": {
                    "weekly": {
                        "json": true
                    }
                }
            }
        }),
        "daily",
        None,
        "daily",
    );

    let mut pi = SharedArgs::default();
    config.for_dependency("pi").apply_shared(&mut pi);
    assert!(pi.json);
    let mut openclaw = SharedArgs::default();
    config
        .for_dependency("openclaw")
        .apply_shared(&mut openclaw);
    assert!(openclaw.offline);
    let mut codex = SharedArgs::default();
    config.for_dependency("codex").apply_shared(&mut codex);
    assert_eq!(codex.mode, CostMode::Display);

    let focused = context(
        json!({
            "codex": {
                "defaults": {
                    "speed": "fast"
                }
            }
        }),
        "codex daily",
        Some("codex"),
        "daily",
    );

    assert_eq!(
        focused
            .for_dependency("codex")
            .agent_options("codex")
            .codex_speed,
        CodexSpeed::Fast
    );
}

#[test]
fn source_view_rejects_invalid_source_date_bounds() {
    let config = context(
        json!({"codex":{"defaults":{"since":"2026-02-30"}}}),
        "daily",
        None,
        "daily",
    );
    let source = config.for_dependency("codex");
    assert_eq!(
        source.config_error(),
        Some(
            "Invalid ccusage config: codex.defaults.since: not a valid date. Expected YYYY-MM-DD or YYYYMMDD"
        )
    );
}

#[test]
fn one_hour_cache_rates_survive_parsing_and_partial_config_merges() {
    let parent: ConfigPricingOverride = serde_json::from_str(
        r#"{"inputCostPerToken":0.000001,"cacheCreationInputTokenCostAbove1hr":0.000007}"#,
    )
    .unwrap();
    let mut current = BTreeMap::from([("model".to_string(), parent.into())]);
    let child: ConfigPricingOverride =
        serde_json::from_str(r#"{"outputCostPerToken":0.000002}"#).unwrap();
    merge_pricing_overrides(&mut current, BTreeMap::from([("model".to_string(), child)]));
    assert_eq!(
        current["model"].cache_creation_input_token_cost_above_1hr,
        Some(7e-6)
    );
    let child: ConfigPricingOverride =
        serde_json::from_str(r#"{"cacheCreationInputTokenCostAbove1hr":0}"#).unwrap();
    merge_pricing_overrides(&mut current, BTreeMap::from([("model".to_string(), child)]));
    assert_eq!(
        current["model"].cache_creation_input_token_cost_above_1hr,
        Some(0.0)
    );
    assert_eq!(current["model"].input_cost_per_token, Some(1e-6));
}

#[test]
fn merge_pricing_overrides_field_level_preserves_parent_fields() {
    use crate::config_schema::ConfigPricingOverride;
    use ccusage_cli::PricingOverride;

    let mut current = BTreeMap::new();
    current.insert(
        "[pi] gpt-5.4".to_string(),
        PricingOverride {
            input_cost_per_token: Some(2.5e-6),
            output_cost_per_token: Some(1.5e-5),
            fast_multiplier: Some(2.0),
            flex_multiplier: Some(0.75),
            ..Default::default()
        },
    );

    let mut incoming = BTreeMap::new();
    incoming.insert(
        "[pi] gpt-5.4".to_string(),
        ConfigPricingOverride {
            max_input_tokens: Some(1_000_000),
            ..Default::default()
        },
    );

    merge_pricing_overrides(&mut current, incoming);

    let result = &current["[pi] gpt-5.4"];
    assert_eq!(result.input_cost_per_token, Some(2.5e-6));
    assert_eq!(result.output_cost_per_token, Some(1.5e-5));
    assert_eq!(result.fast_multiplier, Some(2.0));
    assert_eq!(result.flex_multiplier, Some(0.75));
    assert_eq!(result.max_input_tokens, Some(1_000_000));
}

#[test]
fn merge_pricing_overrides_child_overrides_parent_field() {
    use crate::config_schema::ConfigPricingOverride;
    use ccusage_cli::PricingOverride;

    let mut current = BTreeMap::new();
    current.insert(
        "model-a".to_string(),
        PricingOverride {
            input_cost_per_token: Some(3e-6),
            output_cost_per_token: Some(15e-6),
            cache_read_input_token_cost: Some(3e-7),
            fast_multiplier: Some(2.0),
            flex_multiplier: Some(0.75),
            ..Default::default()
        },
    );

    let mut incoming = BTreeMap::new();
    incoming.insert(
        "model-a".to_string(),
        ConfigPricingOverride {
            input_cost_per_token: Some(2e-6),
            fast_multiplier: Some(3.0),
            flex_multiplier: Some(0.5),
            ..Default::default()
        },
    );

    merge_pricing_overrides(&mut current, incoming);

    let result = &current["model-a"];
    assert_eq!(result.input_cost_per_token, Some(2e-6));
    assert_eq!(result.output_cost_per_token, Some(15e-6));
    assert_eq!(result.cache_read_input_token_cost, Some(3e-7));
    assert_eq!(result.fast_multiplier, Some(3.0));
    assert_eq!(result.flex_multiplier, Some(0.5));
}

#[test]
fn merge_pricing_overrides_preserves_flex_multiplier_for_new_models() {
    let mut current = BTreeMap::new();
    let incoming = BTreeMap::from([(
        "new-model".to_string(),
        ConfigPricingOverride {
            input_cost_per_token: Some(1e-6),
            flex_multiplier: Some(0.5),
            ..Default::default()
        },
    )]);

    merge_pricing_overrides(&mut current, incoming);

    assert_eq!(current["new-model"].input_cost_per_token, Some(1e-6));
    assert_eq!(current["new-model"].flex_multiplier, Some(0.5));
}

#[test]
fn accepts_named_pi_stores_from_config() {
    let config = config_context_from_json(
        r#"{
                "pi": {
                    "stores": [
                        { "name": "omp", "path": "~/.omp/agent/sessions" }
                    ]
                }
            }"#,
    );
    let mut shared = SharedArgs::default();

    assert_eq!(config.config_error(), None);
    apply_config_to_shared(&mut shared, &config);

    assert_eq!(shared.pi_stores.len(), 1);
    assert_eq!(shared.pi_stores[0].name, "omp");
    assert_eq!(shared.pi_stores[0].path, "~/.omp/agent/sessions");
}

#[test]
fn treats_empty_named_pi_store_array_as_no_op() {
    let config = config_context_from_json(r#"{ "pi": { "stores": [] } }"#);
    let mut shared = SharedArgs::default();

    assert_eq!(config.config_error(), None);
    apply_config_to_shared(&mut shared, &config);

    assert!(shared.pi_stores.is_empty());
}

#[test]
fn rejects_named_pi_store_bad_shape() {
    let config = config_context_from_json(r#"{ "pi": { "stores": [{ "name": "omp" }] } }"#);

    assert_config_error(&config, "pi.stores[0]: missing field `path`");
}

#[test]
fn rejects_named_pi_store_invalid_name() {
    let config = config_context_from_json(
        r#"{ "pi": { "stores": [{ "name": "Omp", "path": "/tmp/omp" }] } }"#,
    );

    assert_config_error(
        &config,
        "pi.stores[0].name must match ^[a-z][a-z0-9_-]{0,31}$",
    );
}

#[test]
fn rejects_named_pi_store_builtin_agent_collision() {
    let config = config_context_from_json(
        r#"{ "pi": { "stores": [{ "name": "pi", "path": "/tmp/omp" }] } }"#,
    );

    assert_config_error(
        &config,
        "pi.stores name 'pi' collides with a built-in agent",
    );
}

#[test]
fn rejects_named_pi_store_duplicate_name() {
    let config = config_context_from_json(
        r#"{
                "pi": {
                    "stores": [
                        { "name": "omp", "path": "/tmp/omp-a" },
                        { "name": "omp", "path": "/tmp/omp-b" }
                    ]
                }
            }"#,
    );

    assert_config_error(&config, "duplicate pi.stores name 'omp'");
}

#[test]
fn rejects_named_pi_store_empty_path_with_store_name() {
    let config =
        config_context_from_json(r#"{ "pi": { "stores": [{ "name": "omp", "path": "  " }] } }"#);

    assert_config_error(
        &config,
        "pi.stores[0] ('omp'): path must be a non-empty string",
    );
}

#[test]
fn reserved_named_pi_store_names_match_unified_loader_agents() {
    let reserved = reserved_named_pi_store_names()
        .into_iter()
        .collect::<BTreeSet<_>>();
    let expected = std::iter::once("all")
        .chain(ccusage_cli::BUILT_IN_AGENT_NAMES.iter().copied())
        .collect::<BTreeSet<_>>();

    assert_eq!(reserved, expected);
}

#[test]
fn named_pi_store_validation_uses_schema_pattern() {
    assert_eq!(
        NAMED_PI_STORE_NAME_PATTERN,
        crate::config_schema::NAMED_PI_STORE_NAME_PATTERN
    );
    assert!(validate_named_pi_store_name("omp").is_ok());
    assert!(validate_named_pi_store_name("o3-fork").is_ok());
    assert!(validate_named_pi_store_name("Omp").is_err());
    assert!(validate_named_pi_store_name("all").is_err());
    assert!(validate_named_pi_store_name("codex").is_err());
}

#[test]
fn rejects_config_date_bounds_that_are_not_real_calendar_dates() {
    let config = config_context_from_json(r#"{ "defaults": { "since": "2026-02-30" } }"#);

    assert_config_error(
        &config,
        "defaults.since: not a valid date. Expected YYYY-MM-DD or YYYYMMDD",
    );

    let config = config_context_from_json(r#"{ "commands": { "daily": { "until": "abc" } } }"#);

    assert_config_error(
        &config,
        "commands.daily.until: not a valid date. Expected YYYY-MM-DD or YYYYMMDD",
    );
}

#[test]
fn rejects_config_date_bounds_that_are_not_strings() {
    let config = config_context_from_json(r#"{ "defaults": { "since": 20260710 } }"#);

    assert_config_error(
        &config,
        "defaults.since: invalid type: integer `20260710`, expected a string",
    );
}

#[test]
fn detects_commands_after_root_options_with_values() {
    for command in [
        vec!["--last", "1", "weekly"],
        vec!["--sections", "daily", "weekly"],
    ] {
        let config = config_context_for_command(
            r#"{ "commands": { "weekly": { "since": "not-a-date" } } }"#,
            &command,
        );

        assert_config_error(
            &config,
            "commands.weekly.since: not a valid date. Expected YYYY-MM-DD or YYYYMMDD",
        );
    }
}

#[test]
fn rejects_config_date_bounds_for_commands_without_named_pi_stores() {
    for command in [
        vec!["blocks"],
        vec!["statusline"],
        vec!["all", "daily"],
        vec!["codex", "daily"],
    ] {
        let config =
            config_context_for_command(r#"{ "defaults": { "since": "2026-02-30" } }"#, &command);

        assert_config_error(
            &config,
            "defaults.since: not a valid date. Expected YYYY-MM-DD or YYYYMMDD",
        );
    }
}

#[test]
fn rejects_invalid_explicit_named_stores_for_every_command() {
    let raw = r#"{"pi":{"stores":[{"name":"omp"}]}}"#;
    for command in [vec!["daily"], vec!["blocks"], vec!["codex", "daily"]] {
        let config = config_context_for_command(raw, &command);
        assert_config_error(&config, "pi.stores[0]: missing field `path`");
    }
}

#[test]
fn normalizes_valid_config_date_bounds() {
    let config = config_context_from_json(
        r#"{ "defaults": { "since": "2026-01-01", "until": "20260131" } }"#,
    );
    let mut shared = SharedArgs::default();

    assert_eq!(config.config_error(), None);
    apply_config_to_shared(&mut shared, &config);

    assert_eq!(
        shared.since.map(|date| date.to_string()).as_deref(),
        Some("20260101")
    );
    assert_eq!(
        shared.until.map(|date| date.to_string()).as_deref(),
        Some("20260131")
    );
}

fn user_config_timezone() -> Option<String> {
    let config = ConfigContext::from_args(&["daily".to_string()]);
    config.timezone().map(str::to_owned)
}

#[test]
fn loads_the_user_config_from_xdg_config_home() {
    let fixture = fs_fixture!({
        "xdg/claude/ccusage.json": r#"{ "defaults": { "timezone": "Asia/Tokyo" } }"#,
    });
    Environment::with_vars([
        ("CLAUDE_CONFIG_DIR", None),
        ("HOME", Some(fixture.path("home").into())),
        ("XDG_CONFIG_HOME", Some(fixture.path("xdg").into())),
    ])
    .run(|| {
        assert_eq!(user_config_timezone().as_deref(), Some("Asia/Tokyo"));
    })
}

#[test]
fn loads_the_user_config_from_a_tilde_claude_config_dir_entry() {
    let fixture = fs_fixture!({
        "home/claude-a/.keep": "",
        "home/claude-b/ccusage.json": r#"{ "defaults": { "timezone": "Asia/Tokyo" } }"#,
    });
    Environment::with_vars([
        ("HOME", Some(fixture.path("home").into())),
        ("CLAUDE_CONFIG_DIR", Some("~/claude-a, ~/claude-b".into())),
    ])
    .run(|| {
        assert_eq!(user_config_timezone().as_deref(), Some("Asia/Tokyo"));
    })
}

#[test]
fn loads_the_user_config_beside_a_projects_claude_config_dir_entry() {
    let fixture = fs_fixture!({
        "archive/ccusage.json": r#"{ "defaults": { "timezone": "Asia/Tokyo" } }"#,
        "archive/projects/project/session.jsonl": "",
    });
    Environment::with_vars([(
        "CLAUDE_CONFIG_DIR",
        Some(fixture.path("archive/projects").into()),
    )])
    .run(|| {
        assert_eq!(user_config_timezone().as_deref(), Some("Asia/Tokyo"));
    })
}

fn assert_config_error(config: &ConfigContext, detail: &str) {
    let error = config
        .config_error()
        .expect("invalid explicit config must fail");
    assert!(error.starts_with("Invalid ccusage config: "), "{error}");
    assert!(error.contains("ccusage.json:"), "{error}");
    assert!(error.ends_with(detail), "{error}");
}

fn context(value: Value, raw: &str, agent: Option<&str>, report: &str) -> ConfigContext {
    let parsed = crate::layers::ConfigFile::parse(value.clone());
    let (layers, error, warnings) = match parsed {
        Ok(file) => (file.layers, None, file.warnings),
        Err(error) => (BTreeMap::new(), Some(config_error(error)), Vec::new()),
    };
    ConfigContext {
        layers,
        command: ConfigCommand {
            raw: raw.to_string(),
            agent: agent.map(ToString::to_string),
            report: report.to_string(),
        },
        pi_stores: Vec::new(),
        pi_stores_error: None,
        error,
        warnings: RefCell::new(warnings),
        config_origin: None,
    }
}

fn config_context_from_json(raw: &str) -> ConfigContext {
    config_context_for_command(raw, &["daily"])
}

fn config_context_for_command(raw: &str, command: &[&str]) -> ConfigContext {
    let fixture = fs_fixture!({
        "ccusage.json": raw,
    });
    let mut args = command
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<String>>();
    args.push("--config".to_string());
    args.push(fixture.path("ccusage.json").to_string_lossy().into_owned());
    ConfigContext::from_args(&args)
}
