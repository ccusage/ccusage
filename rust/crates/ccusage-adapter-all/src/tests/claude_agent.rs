use super::*;

const CLAUDE_AGENT_DAY_ONE: i64 = 4_070_995_200_000;

const CLAUDE_AGENT_DAY_TWO: i64 = 4_071_081_600_000;

fn claude_agent_row(native_session: &str, created: i64, input: u64) -> String {
    format!(
        r#"{{"time":{{"created":{created}}},"model":{{"id":"opus","providerID":"claude-agent"}},"content":[{{"type":"text","text":"hi","native":{{"sessionID":"{native_session}","messageID":"m"}}}}],"tokens":{{"input":{input},"output":50}},"cost":0}}"#
    )
}

fn claude_agent_shared(since: &str, until: &str) -> SharedArgs {
    SharedArgs {
        mode: CostMode::Display,
        ..fixture_shared(since, until)
    }
}

fn unified_totals(
    kind: AgentReportKind,
    shared: &SharedArgs,
    sources: &std::collections::BTreeMap<String, SourceArgs>,
) -> Value {
    let result = load_rows(kind, shared, sources, WeekDay::Monday).unwrap();
    report_json_with_agents(&result.rows, kind, false)["totals"].clone()
}

#[test]
fn unified_reports_count_claude_agent_sessions_once() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    fixture.write_file(
        "claude/projects/project-a/claude-native-a.jsonl",
        claude_usage_line("claude-native-a", "2099-01-02T00:00:00.000Z", "msg-a", 100),
    );
    write_opencode_rows(
        &fixture.create_dir_all("opencode").join("opencode.db"),
        &[
            (
                "m-dup",
                "ses-agent",
                CLAUDE_AGENT_DAY_ONE,
                claude_agent_row("claude-native-a", CLAUDE_AGENT_DAY_ONE, 100),
            ),
            (
                "m-no-log",
                "ses-agent-old",
                CLAUDE_AGENT_DAY_ONE,
                claude_agent_row("claude-native-missing", CLAUDE_AGENT_DAY_ONE, 7),
            ),
            (
                "m-api",
                "ses-api",
                CLAUDE_AGENT_DAY_ONE,
                api_row(CLAUDE_AGENT_DAY_ONE, 20),
            ),
        ],
    );
    claude_opencode_env(&fixture, fixture.path("claude").into_os_string()).run(|| {
        let shared = claude_agent_shared("20990101", "20990103");

        for kind in [AgentReportKind::Daily, AgentReportKind::Session] {
            let result = load_rows(kind, &shared, &NO_SOURCES, WeekDay::Monday).unwrap();
            let totals = &report_json_with_agents(&result.rows, kind, false)["totals"];

            assert_eq!(totals["inputTokens"], 127, "{kind:?}");
            assert_eq!(totals["totalCost"], 1.5, "{kind:?}");
            assert_eq!(result.detected_agents, ["claude", "opencode"], "{kind:?}");
        }
    })
}

#[test]
fn unified_project_filter_does_not_restore_opencode_mirrors() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    write_claude_agent_fixture(&fixture);
    fixture.write_file(
        "claude/projects/project-b/claude-native-b.jsonl",
        claude_usage_line("claude-native-b", "2099-01-02T00:00:00.000Z", "msg-b", 10),
    );
    claude_opencode_env(&fixture, fixture.path("claude").into_os_string()).run(|| {
        let shared = claude_agent_shared("20990101", "20990103");
        let sources = BTreeMap::from([(
            "claude".to_string(),
            SourceArgs {
                options: ccusage_core::cli::AgentOptions {
                    claude_project: Some("project-b".to_string()),
                    ..Default::default()
                },
                shared: shared.clone(),
            },
        )]);

        for kind in [
            AgentReportKind::Daily,
            AgentReportKind::Weekly,
            AgentReportKind::Monthly,
            AgentReportKind::Session,
        ] {
            assert_eq!(
                unified_totals(kind, &shared, &sources)["inputTokens"],
                37,
                "{kind:?}"
            );
        }
    })
}

#[test]
fn unified_project_filter_keeps_claims_for_selected_rows_with_later_copies() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    for (project, session, timestamp, input) in [
        ("selected", "native", "2099-01-02T00:00:00Z", 100),
        ("other", "copy", "2099-01-05T00:00:00Z", 200),
    ] {
        fixture.write_file(
            format!("claude/projects/{project}/{session}.jsonl"),
            claude_usage_line(session, timestamp, "same-message", input),
        );
    }
    write_opencode_rows(
        &fixture.create_dir_all("opencode").join("opencode.db"),
        &[(
            "m-mirror",
            "ses-agent",
            CLAUDE_AGENT_DAY_ONE,
            claude_agent_row("native", CLAUDE_AGENT_DAY_ONE, 100),
        )],
    );
    claude_opencode_env(&fixture, fixture.path("claude").into_os_string()).run(|| {
        let shared = claude_agent_shared("20990101", "20990103");
        let sources = BTreeMap::from([(
            "claude".to_string(),
            SourceArgs {
                options: ccusage_core::cli::AgentOptions {
                    claude_project: Some("selected".to_string()),
                    ..Default::default()
                },
                shared: shared.clone(),
            },
        )]);
        for kind in [AgentReportKind::Daily, AgentReportKind::Session] {
            assert_eq!(
                unified_totals(kind, &shared, &sources)["inputTokens"],
                100,
                "{kind:?}"
            );
        }
    });
}

#[test]
fn unbounded_claims_follow_recorded_ids_even_when_the_file_stem_differs() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    fixture.write_file(
        "claude/projects/selected/own.jsonl",
        claude_usage_line("own", "2099-01-02T00:00:00Z", "msg-own", 10),
    );
    fixture.write_file(
        "claude/projects/other/stem.jsonl",
        claude_usage_line("native", "2099-01-02T00:00:00Z", "msg-native", 100),
    );
    write_opencode_rows(
        &fixture.create_dir_all("opencode").join("opencode.db"),
        &[
            (
                "m-native",
                "ses-native",
                CLAUDE_AGENT_DAY_ONE,
                claude_agent_row("native", CLAUDE_AGENT_DAY_ONE, 100),
            ),
            (
                "m-stem",
                "ses-stem",
                CLAUDE_AGENT_DAY_ONE,
                claude_agent_row("stem", CLAUDE_AGENT_DAY_ONE, 7),
            ),
        ],
    );
    claude_opencode_env(&fixture, fixture.path("claude").into_os_string()).run(|| {
        let shared = SharedArgs {
            since: None,
            until: None,
            ..claude_agent_shared("20990101", "20990103")
        };
        let source = SourceArgs {
            options: ccusage_core::cli::AgentOptions {
                claude_project: Some("selected".to_string()),
                ..Default::default()
            },
            shared: shared.clone(),
        };
        let sources = BTreeMap::from([("claude".to_string(), source.clone())]);
        for kind in [AgentReportKind::Daily, AgentReportKind::Session] {
            let (focused, _) =
                loader::load_opencode_entries(&shared, kind, false, Some(&source)).unwrap();
            assert_eq!(focused.len(), 1, "{kind:?}");
            assert_eq!(focused[0].usage.input_tokens, 7, "{kind:?}");
            assert_eq!(
                unified_totals(kind, &shared, &sources)["inputTokens"],
                17,
                "{kind:?}"
            );
        }
    });
}

fn focused_opencode_input(
    shared: &SharedArgs,
    kind: AgentReportKind,
    include_claude_agent: bool,
) -> u64 {
    let (mut entries, _) =
        loader::load_opencode_entries(shared, kind, include_claude_agent, None).unwrap();
    filter_loaded_entries_by_date(&mut entries, shared);
    entries.iter().map(|entry| entry.usage.input_tokens).sum()
}

fn write_claude_agent_fixture(fixture: &ccusage_test_support::Fixture) {
    fixture.write_file(
        "claude/projects/project-a/claude-native-a.jsonl",
        claude_usage_line("claude-native-a", "2099-01-02T00:00:00.000Z", "msg-a", 100),
    );
    write_opencode_rows(
        &fixture.create_dir_all("opencode").join("opencode.db"),
        &[
            (
                "m-dup",
                "ses-agent",
                CLAUDE_AGENT_DAY_ONE,
                claude_agent_row("claude-native-a", CLAUDE_AGENT_DAY_ONE, 100),
            ),
            (
                "m-no-log",
                "ses-agent-old",
                CLAUDE_AGENT_DAY_ONE,
                claude_agent_row("claude-native-missing", CLAUDE_AGENT_DAY_ONE, 7),
            ),
            (
                "m-api",
                "ses-api",
                CLAUDE_AGENT_DAY_ONE,
                api_row(CLAUDE_AGENT_DAY_ONE, 20),
            ),
        ],
    );
}

#[test]
fn focused_opencode_reports_leave_claude_agent_turns_to_claude() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    write_claude_agent_fixture(&fixture);
    claude_opencode_env(&fixture, fixture.path("claude").into_os_string()).run(|| {
        let shared = claude_agent_shared("20990101", "20990103");

        for kind in [
            AgentReportKind::Daily,
            AgentReportKind::Weekly,
            AgentReportKind::Monthly,
            AgentReportKind::Session,
        ] {
            assert_eq!(focused_opencode_input(&shared, kind, false), 27, "{kind:?}");
            assert_eq!(focused_opencode_input(&shared, kind, true), 127, "{kind:?}");
            assert_eq!(
                unified_totals(kind, &shared, &NO_SOURCES)["inputTokens"],
                100 + focused_opencode_input(&shared, kind, false),
                "{kind:?}"
            );
        }
    })
}

#[test]
fn focused_opencode_claims_respect_the_claude_window_across_projects() {
    let fixture = fs_fixture!({});
    write_claude_agent_fixture(&fixture);
    claude_opencode_env(&fixture, fixture.path("claude").into_os_string()).run(|| {
        let shared = claude_agent_shared("20990101", "20990103");
        for (source, expected) in [
            (
                SourceArgs {
                    shared: claude_agent_shared("20990103", "20990103"),
                    ..Default::default()
                },
                127,
            ),
            (
                SourceArgs {
                    options: ccusage_core::cli::AgentOptions {
                        claude_project: Some("another-project".to_string()),
                        ..Default::default()
                    },
                    shared: shared.clone(),
                },
                27,
            ),
        ] {
            for kind in [
                AgentReportKind::Daily,
                AgentReportKind::Weekly,
                AgentReportKind::Monthly,
                AgentReportKind::Session,
            ] {
                let (entries, _) =
                    loader::load_opencode_entries(&shared, kind, false, Some(&source)).unwrap();
                assert_eq!(
                    entries
                        .iter()
                        .map(|entry| entry.usage.input_tokens)
                        .sum::<u64>(),
                    expected,
                    "{kind:?}"
                );
            }
        }
    })
}

#[test]
fn focused_opencode_reports_what_the_claude_cross_check_skipped() {
    let fixture = fs_fixture!({});
    write_claude_agent_fixture(&fixture);
    fixture.write_file(
        "claude/projects/project-a/claude-native-missing.jsonl",
        r#"{"timestamp":"2099-01-02T00:00:00Z","message":{"usage":{"input_tokens":"bad"}}}"#,
    );
    claude_opencode_env(&fixture, fixture.path("claude").into_os_string()).run(|| {
        let shared = claude_agent_shared("20990101", "20990103");

        let (_, claude_diagnostics) =
            loader::load_opencode_entries(&shared, AgentReportKind::Daily, false, None).unwrap();
        let (_, included) =
            loader::load_opencode_entries(&shared, AgentReportKind::Daily, true, None).unwrap();

        assert_eq!(
            claude_diagnostics.unwrap().warnings(&shared),
            ["WARN  claude: skipped 1 record in 1 input (invalid data)"]
        );
        assert!(
            included.is_none(),
            "no cross-check runs, so nothing to report"
        );
    })
}

#[test]
fn focused_opencode_session_skips_turns_of_a_claude_session_active_past_until() {
    let fixture = fs_fixture!({});
    fixture.write_file(
        "claude/projects/project-a/claude-native-a.jsonl",
        [
            claude_usage_line("claude-native-a", "2099-01-02T00:00:00.000Z", "msg-a", 100),
            claude_usage_line("claude-native-a", "2099-01-03T00:00:00.000Z", "msg-b", 50),
        ]
        .join("\n"),
    );
    write_opencode_rows(
        &fixture.create_dir_all("opencode").join("opencode.db"),
        &[
            (
                "m-day-one",
                "ses-agent",
                CLAUDE_AGENT_DAY_ONE,
                claude_agent_row("claude-native-a", CLAUDE_AGENT_DAY_ONE, 100),
            ),
            (
                "m-day-two",
                "ses-agent",
                CLAUDE_AGENT_DAY_TWO,
                claude_agent_row("claude-native-a", CLAUDE_AGENT_DAY_TWO, 50),
            ),
        ],
    );
    claude_opencode_env(&fixture, fixture.path("claude").into_os_string()).run(|| {
        let shared = claude_agent_shared("20990101", "20990102");
        let mut claude_entries = adapter::claude::load_entries(&shared, None).unwrap();
        filter_loaded_entries_by_date(&mut claude_entries, &shared);

        assert_eq!(claude_entries.len(), 1);
        for kind in [AgentReportKind::Daily, AgentReportKind::Session] {
            assert_eq!(focused_opencode_input(&shared, kind, false), 0, "{kind:?}");
            assert_eq!(focused_opencode_input(&shared, kind, true), 100, "{kind:?}");
        }
    })
}

#[test]
fn focused_opencode_reports_keep_claude_agent_turns_without_a_claude_source() {
    let fixture = fs_fixture!({});
    write_claude_agent_fixture(&fixture);
    isolated_agents_env(
        &fixture,
        [(
            "OPENCODE_DATA_DIR",
            fixture.path("opencode").into_os_string(),
        )],
    )
    .run(|| {
        let shared = claude_agent_shared("20990101", "20990103");

        for kind in [AgentReportKind::Daily, AgentReportKind::Session] {
            assert_eq!(
                focused_opencode_input(&shared, kind, false),
                127,
                "{kind:?}"
            );
        }
    })
}

#[test]
fn focused_opencode_reports_fail_like_unified_reports_on_claude_errors() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    write_claude_agent_fixture(&fixture);
    claude_opencode_env(&fixture, fixture.path("missing-claude").into_os_string()).run(|| {
        let shared = claude_agent_shared("20990101", "20990103");

        for kind in [AgentReportKind::Daily, AgentReportKind::Session] {
            assert!(
                loader::load_opencode_entries(&shared, kind, false, None).is_err(),
                "{kind:?}"
            );
            assert!(
                load_rows(kind, &shared, &NO_SOURCES, WeekDay::Monday).is_err(),
                "{kind:?}"
            );
            assert!(
                loader::load_opencode_entries(&shared, kind, true, None).is_ok(),
                "{kind:?}"
            );
        }
    })
}

#[test]
fn unified_reports_keep_claude_agent_rows_whose_transcript_has_no_usage() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "claude/projects/project-a/claude-native-a.jsonl": r#"{"timestamp":"2099-01-02T00:00:00.000Z","sessionId":"claude-native-a","type":"user","message":{"role":"user","content":"hi"}}"#,
    });
    write_opencode_rows(
        &fixture.create_dir_all("opencode").join("opencode.db"),
        &[(
            "m-agent",
            "ses-agent",
            CLAUDE_AGENT_DAY_ONE,
            claude_agent_row("claude-native-a", CLAUDE_AGENT_DAY_ONE, 100),
        )],
    );
    claude_opencode_env(&fixture, fixture.path("claude").into_os_string()).run(|| {
        let shared = claude_agent_shared("20990101", "20990103");

        for kind in [AgentReportKind::Daily, AgentReportKind::Session] {
            assert_eq!(
                unified_totals(kind, &shared, &NO_SOURCES)["inputTokens"],
                100,
                "{kind:?}"
            );
        }
    })
}

#[test]
fn unified_reports_keep_claude_agent_rows_whose_transcript_holds_only_subagent_usage() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    fixture.write_file(
        "claude/projects/project-a/claude-native-a/subagents/agent-1.jsonl",
        claude_usage_line("claude-native-a", "2099-01-02T00:00:00.000Z", "msg-sub", 30).replacen(
            '{',
            r#"{"isSidechain":true,"#,
            1,
        ),
    );
    write_opencode_rows(
        &fixture.create_dir_all("opencode").join("opencode.db"),
        &[(
            "m-agent",
            "ses-agent",
            CLAUDE_AGENT_DAY_ONE,
            claude_agent_row("claude-native-a", CLAUDE_AGENT_DAY_ONE, 100),
        )],
    );
    claude_opencode_env(&fixture, fixture.path("claude").into_os_string()).run(|| {
        let shared = claude_agent_shared("20990101", "20990103");

        for kind in [AgentReportKind::Daily, AgentReportKind::Session] {
            assert_eq!(
                unified_totals(kind, &shared, &NO_SOURCES)["inputTokens"],
                130,
                "{kind:?}"
            );
        }
    })
}

#[test]
fn unified_daily_keeps_claude_agent_rows_on_days_the_claude_window_excludes() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    fixture.write_file(
        "claude/projects/project-a/claude-native-a.jsonl",
        claude_usage_line("claude-native-a", "2099-01-02T00:00:00.000Z", "msg-a", 100),
    );
    write_opencode_rows(
        &fixture.create_dir_all("opencode").join("opencode.db"),
        &[(
            "m-dup",
            "ses-agent",
            CLAUDE_AGENT_DAY_ONE,
            claude_agent_row("claude-native-a", CLAUDE_AGENT_DAY_ONE, 100),
        )],
    );
    claude_opencode_env(&fixture, fixture.path("claude").into_os_string()).run(|| {
        let shared = claude_agent_shared("20990101", "20990103");
        let sources = std::collections::BTreeMap::from([(
            "claude".to_string(),
            SourceArgs {
                shared: SharedArgs {
                    since: Some("20990103".parse().unwrap()),
                    ..shared.clone()
                },
                ..SourceArgs::default()
            },
        )]);

        assert_eq!(
            unified_totals(AgentReportKind::Daily, &shared, &sources)["inputTokens"],
            100
        );
    })
}

#[test]
fn unified_daily_keeps_turns_when_native_usage_is_before_since() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    fixture.write_file(
        "claude/projects/project-a/claude-native-a.jsonl",
        claude_usage_line("claude-native-a", "2099-01-02T00:00:00.000Z", "msg-a", 100),
    );
    write_opencode_rows(
        &fixture.create_dir_all("opencode").join("opencode.db"),
        &[
            (
                "m-dup",
                "ses-agent",
                CLAUDE_AGENT_DAY_ONE,
                claude_agent_row("claude-native-a", CLAUDE_AGENT_DAY_ONE, 100),
            ),
            (
                "m-later",
                "ses-agent",
                CLAUDE_AGENT_DAY_TWO,
                claude_agent_row("claude-native-a", CLAUDE_AGENT_DAY_TWO, 7),
            ),
        ],
    );
    claude_opencode_env(&fixture, fixture.path("claude").into_os_string()).run(|| {
        assert_eq!(
            unified_totals(
                AgentReportKind::Daily,
                &claude_agent_shared("20990103", "20990103"),
                &NO_SOURCES
            )["inputTokens"],
            7
        );
    })
}

#[test]
fn unified_reports_keep_api_turns_when_the_native_session_is_claimed() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    fixture.write_file(
        "claude/projects/project-a/claude-native-a.jsonl",
        claude_usage_line("claude-native-a", "2099-01-02T00:00:00.000Z", "msg-a", 100),
    );
    write_opencode_rows(
        &fixture.create_dir_all("opencode").join("opencode.db"),
        &[
            (
                "m-dup",
                "ses-mixed",
                CLAUDE_AGENT_DAY_ONE,
                claude_agent_row("claude-native-a", CLAUDE_AGENT_DAY_ONE, 100),
            ),
            (
                "m-api",
                "ses-mixed",
                CLAUDE_AGENT_DAY_ONE,
                api_row(CLAUDE_AGENT_DAY_ONE, 20),
            ),
            (
                "m-later",
                "ses-mixed",
                CLAUDE_AGENT_DAY_TWO,
                claude_agent_row("claude-native-a", CLAUDE_AGENT_DAY_TWO, 7),
            ),
        ],
    );
    claude_opencode_env(&fixture, fixture.path("claude").into_os_string()).run(|| {
        let shared = claude_agent_shared("20990101", "20990103");

        for (kind, input) in [
            (AgentReportKind::Daily, 120),
            (AgentReportKind::Session, 120),
        ] {
            assert_eq!(
                unified_totals(kind, &shared, &NO_SOURCES)["inputTokens"],
                input,
                "{kind:?}"
            );
        }
    })
}

#[test]
fn unified_reports_match_claude_agent_rows_against_every_claude_config_root() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    fixture.write_file(
        "claude-a/projects/project-a/other-session.jsonl",
        claude_usage_line("other-session", "2099-01-02T00:00:00.000Z", "msg-other", 10),
    );
    fixture.write_file(
        "claude-b/projects/project-b/claude-native-b.jsonl",
        claude_usage_line("claude-native-b", "2099-01-02T00:00:00.000Z", "msg-b", 100),
    );
    write_opencode_rows(
        &fixture.create_dir_all("opencode").join("opencode.db"),
        &[(
            "m-dup",
            "ses-agent",
            CLAUDE_AGENT_DAY_ONE,
            claude_agent_row("claude-native-b", CLAUDE_AGENT_DAY_ONE, 100),
        )],
    );
    claude_opencode_env(
        &fixture,
        OsString::from(format!(
            "{},{}",
            fixture.path("claude-a").display(),
            fixture.path("claude-b").display()
        )),
    )
    .run(|| {
        let shared = claude_agent_shared("20990101", "20990103");

        for kind in [AgentReportKind::Daily, AgentReportKind::Session] {
            assert_eq!(
                unified_totals(kind, &shared, &NO_SOURCES)["inputTokens"],
                110,
                "{kind:?}"
            );
        }
    })
}

#[test]
fn unified_daily_dates_claude_agent_rows_in_the_claude_source_timezone() {
    ccusage_test_support::install_pricing_snapshots();
    let tokyo_day_three = CLAUDE_AGENT_DAY_ONE + 20 * 3_600_000;
    let tokyo_day_four = CLAUDE_AGENT_DAY_TWO + 16 * 3_600_000;
    let fixture = fs_fixture!({});
    fixture.write_file(
        "claude/projects/project-a/claude-native-a.jsonl",
        claude_usage_line("claude-native-a", "2099-01-02T20:00:00.000Z", "msg-a", 100),
    );
    write_opencode_rows(
        &fixture.create_dir_all("opencode").join("opencode.db"),
        &[
            (
                "m-dup",
                "ses-agent",
                tokyo_day_three,
                claude_agent_row("claude-native-a", tokyo_day_three, 100),
            ),
            (
                "m-next-tokyo-day",
                "ses-agent",
                tokyo_day_four,
                claude_agent_row("claude-native-a", tokyo_day_four, 7),
            ),
        ],
    );
    claude_opencode_env(&fixture, fixture.path("claude").into_os_string()).run(|| {
        let shared = claude_agent_shared("20990101", "20990105");
        let sources = std::collections::BTreeMap::from([(
            "claude".to_string(),
            SourceArgs {
                shared: SharedArgs {
                    timezone: Some("Asia/Tokyo".parse().unwrap()),
                    ..shared.clone()
                },
                ..SourceArgs::default()
            },
        )]);

        assert_eq!(
            unified_totals(AgentReportKind::Daily, &shared, &sources)["inputTokens"],
            100
        );
    })
}

#[test]
fn unified_reports_count_a_claude_agent_turn_once_when_claude_logs_it_after_midnight() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    fixture.write_file(
        "claude/projects/project-a/claude-native-a.jsonl",
        claude_usage_line("claude-native-a", "2099-01-03T00:00:05.000Z", "msg-a", 100),
    );
    let turn_start = CLAUDE_AGENT_DAY_TWO - 10_000;
    write_opencode_rows(
        &fixture.create_dir_all("opencode").join("opencode.db"),
        &[(
            "m-dup",
            "ses-agent",
            turn_start,
            claude_agent_row("claude-native-a", turn_start, 100),
        )],
    );
    claude_opencode_env(&fixture, fixture.path("claude").into_os_string()).run(|| {
        let shared = claude_agent_shared("20990101", "20990104");

        for kind in [
            AgentReportKind::Daily,
            AgentReportKind::Weekly,
            AgentReportKind::Monthly,
            AgentReportKind::Session,
        ] {
            assert_eq!(
                unified_totals(kind, &shared, &NO_SOURCES)["inputTokens"],
                100,
                "{kind:?}"
            );
        }
    })
}

#[test]
fn a_claude_agent_turn_before_until_does_not_borrow_next_days_usage() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    fixture.write_file(
        "claude/projects/project-a/claude-native-a.jsonl",
        claude_usage_line("claude-native-a", "2099-01-03T00:00:05.000Z", "msg-a", 100),
    );
    let turn_start = CLAUDE_AGENT_DAY_TWO - 10_000;
    write_opencode_rows(
        &fixture.create_dir_all("opencode").join("opencode.db"),
        &[(
            "m-dup",
            "ses-agent",
            turn_start,
            claude_agent_row("claude-native-a", turn_start, 100),
        )],
    );
    claude_opencode_env(&fixture, fixture.path("claude").into_os_string()).run(|| {
        let shared = claude_agent_shared("20990102", "20990102");
        for kind in [
            AgentReportKind::Daily,
            AgentReportKind::Weekly,
            AgentReportKind::Monthly,
            AgentReportKind::Session,
        ] {
            assert_eq!(
                unified_totals(kind, &shared, &NO_SOURCES)["inputTokens"],
                0,
                "{kind:?}"
            );
            let (entries, _) = loader::load_opencode_entries(&shared, kind, false, None).unwrap();
            assert!(entries.is_empty(), "{kind:?}");
            let (entries, _) = loader::load_opencode_entries(&shared, kind, true, None).unwrap();
            assert_eq!(entries.len(), 1, "{kind:?}");
        }
    })
}

#[test]
fn native_claims_widen_by_calendar_days_across_months_and_clock_changes() {
    ccusage_test_support::install_pricing_snapshots();
    for (timezone, day, call, turn) in [
        (
            "UTC",
            "20990131",
            "2099-02-01T00:00:05Z",
            "2099-01-31T23:59:50Z",
        ),
        (
            "America/New_York",
            "20261101",
            "2026-11-02T05:00:05Z",
            "2026-11-01T04:30:00Z",
        ),
    ] {
        let fixture = fs_fixture!({});
        fixture.write_file(
            "claude/projects/p/native.jsonl",
            claude_usage_line("native", call, "m", 100),
        );
        let created = ccusage_core::parse_ts_timestamp(turn).unwrap().as_millis();
        write_opencode_rows(
            &fixture.create_dir_all("opencode").join("opencode.db"),
            &[(
                "m-dup",
                "ses-agent",
                created,
                claude_agent_row("native", created, 100),
            )],
        );
        claude_opencode_env(&fixture, fixture.path("claude").into_os_string()).run(|| {
            let shared = SharedArgs {
                timezone: Some(timezone.parse().unwrap()),
                ..claude_agent_shared(day, day)
            };
            for kind in [AgentReportKind::Daily, AgentReportKind::Session] {
                assert_eq!(
                    unified_totals(kind, &shared, &NO_SOURCES)["inputTokens"],
                    0,
                    "{timezone} {kind:?}"
                );
                let (entries, _) =
                    loader::load_opencode_entries(&shared, kind, false, None).unwrap();
                assert!(entries.is_empty(), "{timezone} {kind:?}");
            }
        })
    }
}

#[test]
fn native_claims_keep_turns_when_claude_usage_is_beyond_the_one_day_margin() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    fixture.write_file(
        "claude/projects/p/native.jsonl",
        claude_usage_line("native", "2099-01-02T00:00:05Z", "m", 100),
    );
    let created = ccusage_core::parse_ts_timestamp("2099-01-04T12:00:00Z")
        .unwrap()
        .as_millis();
    write_opencode_rows(
        &fixture.create_dir_all("opencode").join("opencode.db"),
        &[(
            "m-kept",
            "ses-agent",
            created,
            claude_agent_row("native", created, 7),
        )],
    );
    claude_opencode_env(&fixture, fixture.path("claude").into_os_string()).run(|| {
        let shared = claude_agent_shared("20990104", "20990104");
        for kind in [AgentReportKind::Daily, AgentReportKind::Session] {
            assert_eq!(
                unified_totals(kind, &shared, &NO_SOURCES)["inputTokens"],
                7,
                "{kind:?}"
            );
            let (entries, _) = loader::load_opencode_entries(&shared, kind, false, None).unwrap();
            assert_eq!(entries.len(), 1, "{kind:?}");
        }
    })
}

#[test]
fn unified_daily_drops_a_claude_agent_turn_whose_later_calls_fall_after_until() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    fixture.write_file(
        "claude/projects/project-a/claude-native-a.jsonl",
        [
            claude_usage_line("claude-native-a", "2099-01-02T23:59:55.000Z", "msg-a", 100),
            claude_usage_line("claude-native-a", "2099-01-03T00:00:05.000Z", "msg-b", 40),
        ]
        .join("\n"),
    );
    let turn_start = CLAUDE_AGENT_DAY_TWO - 10_000;
    write_opencode_rows(
        &fixture.create_dir_all("opencode").join("opencode.db"),
        &[(
            "m-dup",
            "ses-agent",
            turn_start,
            claude_agent_row("claude-native-a", turn_start, 140),
        )],
    );
    claude_opencode_env(&fixture, fixture.path("claude").into_os_string()).run(|| {
        assert_eq!(
            unified_totals(
                AgentReportKind::Daily,
                &claude_agent_shared("20990101", "20990102"),
                &NO_SOURCES
            )["inputTokens"],
            100
        );
    })
}

#[test]
fn unified_session_counts_a_claude_agent_session_active_past_until_once() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    fixture.write_file(
        "claude/projects/project-a/claude-native-a.jsonl",
        [
            claude_usage_line("claude-native-a", "2099-01-02T00:00:00.000Z", "msg-a", 100),
            claude_usage_line("claude-native-a", "2099-01-03T00:00:00.000Z", "msg-b", 10),
        ]
        .join("\n"),
    );
    write_opencode_rows(
        &fixture.create_dir_all("opencode").join("opencode.db"),
        &[(
            "m-dup",
            "ses-agent",
            CLAUDE_AGENT_DAY_ONE,
            claude_agent_row("claude-native-a", CLAUDE_AGENT_DAY_ONE, 100),
        )],
    );
    claude_opencode_env(&fixture, fixture.path("claude").into_os_string()).run(|| {
        let shared = claude_agent_shared("20990101", "20990102");

        for (kind, period) in [
            (AgentReportKind::Daily, "2099-01-02"),
            (AgentReportKind::Session, "claude-native-a"),
        ] {
            let result = load_rows(kind, &shared, &NO_SOURCES, WeekDay::Monday).unwrap();
            let rows: Vec<_> = result
                .rows
                .iter()
                .map(|row| {
                    let agents = row
                        .agents()
                        .map(<[_]>::to_vec)
                        .unwrap_or_else(|| vec![row.agent()]);
                    (agents, row.period.as_str(), row.input_tokens)
                })
                .collect();
            assert_eq!(rows, [(vec!["claude"], period, 100)], "{kind:?}");
        }
    })
}

#[test]
fn unified_session_skips_claude_agent_rows_of_a_counted_session_before_the_claude_window() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    fixture.write_file(
        "claude/projects/project-a/claude-native-a.jsonl",
        [
            claude_usage_line("claude-native-a", "2099-01-02T00:00:00.000Z", "msg-a", 100),
            claude_usage_line("claude-native-a", "2099-01-03T00:00:00.000Z", "msg-b", 10),
        ]
        .join("\n"),
    );
    write_opencode_rows(
        &fixture.create_dir_all("opencode").join("opencode.db"),
        &[(
            "m-dup",
            "ses-agent",
            CLAUDE_AGENT_DAY_ONE,
            claude_agent_row("claude-native-a", CLAUDE_AGENT_DAY_ONE, 100),
        )],
    );
    claude_opencode_env(&fixture, fixture.path("claude").into_os_string()).run(|| {
        let shared = claude_agent_shared("20990101", "20990104");
        let sources = std::collections::BTreeMap::from([(
            "claude".to_string(),
            SourceArgs {
                shared: SharedArgs {
                    since: Some("20990103".parse().unwrap()),
                    ..shared.clone()
                },
                ..SourceArgs::default()
            },
        )]);

        assert_eq!(
            unified_totals(AgentReportKind::Session, &shared, &sources)["inputTokens"],
            110
        );
    })
}

#[test]
fn unified_reports_match_claude_agent_rows_against_sessions_holding_deduplicated_copies() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    fixture.write_file(
        "claude/projects/project-a/claude-native-a.jsonl",
        claude_usage_line("claude-native-a", "2099-01-02T00:00:00.000Z", "msg-a", 100),
    );
    fixture.write_file(
        "claude/projects/project-a/claude-native-fork.jsonl",
        claude_usage_line(
            "claude-native-fork",
            "2099-01-02T00:00:00.000Z",
            "msg-a",
            100,
        ),
    );
    write_opencode_rows(
        &fixture.create_dir_all("opencode").join("opencode.db"),
        &[
            (
                "m-original",
                "ses-agent",
                CLAUDE_AGENT_DAY_ONE,
                claude_agent_row("claude-native-a", CLAUDE_AGENT_DAY_ONE, 100),
            ),
            (
                "m-fork",
                "ses-agent-fork",
                CLAUDE_AGENT_DAY_ONE,
                claude_agent_row("claude-native-fork", CLAUDE_AGENT_DAY_ONE, 100),
            ),
        ],
    );
    claude_opencode_env(&fixture, fixture.path("claude").into_os_string()).run(|| {
        let shared = claude_agent_shared("20990101", "20990103");

        for kind in [AgentReportKind::Daily, AgentReportKind::Session] {
            assert_eq!(
                unified_totals(kind, &shared, &NO_SOURCES)["inputTokens"],
                100,
                "{kind:?}"
            );
        }
    })
}

#[test]
fn unified_reports_keep_claude_agent_rows_of_a_session_the_claude_window_leaves_out() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    fixture.write_file(
        "claude/projects/project-a/claude-native-a.jsonl",
        claude_usage_line("claude-native-a", "2099-01-02T00:00:00.000Z", "msg-a", 100),
    );
    fixture.write_file(
        "claude/projects/project-a/claude-native-b.jsonl",
        [
            claude_usage_line("claude-native-b", "2099-01-02T00:00:00.000Z", "msg-a", 100),
            claude_usage_line("claude-native-b", "2099-01-03T00:00:00.000Z", "msg-own", 7),
            claude_usage_line("claude-native-b", "2099-01-05T00:00:00.000Z", "msg-late", 1),
        ]
        .join("\n"),
    );
    write_opencode_rows(
        &fixture.create_dir_all("opencode").join("opencode.db"),
        &[(
            "m-own",
            "ses-agent-b",
            CLAUDE_AGENT_DAY_TWO,
            claude_agent_row("claude-native-b", CLAUDE_AGENT_DAY_TWO, 7),
        )],
    );
    claude_opencode_env(&fixture, fixture.path("claude").into_os_string()).run(|| {
        let shared = claude_agent_shared("20990101", "20990106");
        let sources = std::collections::BTreeMap::from([(
            "claude".to_string(),
            SourceArgs {
                shared: SharedArgs {
                    until: Some("20990103".parse().unwrap()),
                    ..shared.clone()
                },
                ..SourceArgs::default()
            },
        )]);

        for kind in [AgentReportKind::Daily, AgentReportKind::Session] {
            assert_eq!(
                unified_totals(kind, &shared, &sources)["inputTokens"],
                107,
                "{kind:?}"
            );
        }
    })
}

#[test]
fn unified_reports_skip_claude_agent_rows_of_a_fork_counted_without_its_parent() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    fixture.write_file(
        "claude/projects/project-a/claude-native-a.jsonl",
        claude_usage_line("claude-native-a", "2099-01-02T00:00:00.000Z", "msg-a", 100),
    );
    fixture.write_file(
        "claude/projects/project-a/claude-native-b.jsonl",
        [
            claude_usage_line("claude-native-b", "2099-01-02T00:00:00.000Z", "msg-a", 100),
            claude_usage_line("claude-native-b", "2099-01-03T00:00:00.000Z", "msg-own", 7),
        ]
        .join("\n"),
    );
    write_opencode_rows(
        &fixture.create_dir_all("opencode").join("opencode.db"),
        &[(
            "m-own",
            "ses-agent-b",
            CLAUDE_AGENT_DAY_TWO,
            claude_agent_row("claude-native-b", CLAUDE_AGENT_DAY_TWO, 7),
        )],
    );
    claude_opencode_env(&fixture, fixture.path("claude").into_os_string()).run(|| {
        let shared = claude_agent_shared("20990101", "20990106");
        let sources = std::collections::BTreeMap::from([(
            "claude".to_string(),
            SourceArgs {
                shared: SharedArgs {
                    since: Some("20990103".parse().unwrap()),
                    ..shared.clone()
                },
                ..SourceArgs::default()
            },
        )]);

        for kind in [AgentReportKind::Daily, AgentReportKind::Session] {
            assert_eq!(
                unified_totals(kind, &shared, &sources)["inputTokens"],
                7,
                "{kind:?}"
            );
        }
    })
}

#[test]
fn unified_reports_match_claude_agent_rows_by_the_recorded_claude_session_id() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    fixture.write_file(
        "claude/projects/project-a/renamed-transcript.jsonl",
        claude_usage_line("claude-native-a", "2099-01-02T00:00:00.000Z", "msg-a", 100),
    );
    write_opencode_rows(
        &fixture.create_dir_all("opencode").join("opencode.db"),
        &[(
            "m-dup",
            "ses-agent",
            CLAUDE_AGENT_DAY_ONE,
            claude_agent_row("claude-native-a", CLAUDE_AGENT_DAY_ONE, 100),
        )],
    );
    claude_opencode_env(&fixture, fixture.path("claude").into_os_string()).run(|| {
        let shared = claude_agent_shared("20990101", "20990103");

        for kind in [AgentReportKind::Daily, AgentReportKind::Session] {
            assert_eq!(
                unified_totals(kind, &shared, &NO_SOURCES)["inputTokens"],
                100,
                "{kind:?}"
            );
        }
    })
}
