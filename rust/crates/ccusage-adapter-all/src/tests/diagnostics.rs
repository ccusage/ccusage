use super::*;

#[test]
fn an_undatable_daily_row_is_skipped_with_a_diagnostic_naming_its_source() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({});
    fixture.write_file(
        "claude/projects/project-a/claude-native-a.jsonl",
        claude_usage_line("claude-native-a", "2099-01-02T00:00:00.000Z", "msg-a", 100),
    );
    let microseconds = 1_700_000_000_000_000;
    write_opencode_rows(
        &fixture.create_dir_all("opencode").join("opencode.db"),
        &[("m-api", "ses-api", microseconds, api_row(microseconds, 20))],
    );
    claude_opencode_env(&fixture, fixture.path("claude").into_os_string()).run(|| {
        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };

        for kind in [
            AgentReportKind::Daily,
            AgentReportKind::Weekly,
            AgentReportKind::Monthly,
        ] {
            let result = load_rows(kind, &shared, &NO_SOURCES, WeekDay::Monday).unwrap();
            let totals = &report_json_with_agents(&result.rows, kind, false)["totals"];
            assert_eq!(totals["inputTokens"], 100, "{kind:?}");
            let opencode = result
                .diagnostics
                .iter()
                .find(|diagnostics| diagnostics.source() == "opencode")
                .unwrap();
            assert_eq!(opencode.skipped(SkipKind::InvalidTimestamp), 1, "{kind:?}");
        }
    })
}

#[test]
fn unified_reports_return_antigravity_skip_warnings_once_instead_of_printing() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "antigravity/conversations/not-a-database.db": "not sqlite",
    });
    isolated_agent_env(
        &fixture,
        "ANTIGRAVITY_DATA_DIR",
        fixture.path("antigravity").into_os_string(),
    )
    .run(|| {
        let shared = fixture_shared("20250615", "20250615");
        let expected = vec!["WARN  antigravity: skipped 1 input (invalid data)".to_string()];

        let rows = loader::load_rows(
            AgentReportKind::Daily,
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        )
        .unwrap();
        let sections = loader::load_sections(
            &[AgentReportKind::Daily, AgentReportKind::Session],
            &shared,
            &NO_SOURCES,
            WeekDay::Monday,
        )
        .unwrap();

        let warnings = |diagnostics: &[Diagnostics]| {
            diagnostics
                .iter()
                .flat_map(|diagnostics| diagnostics.warnings(&shared))
                .collect::<Vec<_>>()
        };
        assert!(rows.rows.is_empty());
        assert_eq!(warnings(&rows.diagnostics), expected);
        assert_eq!(warnings(&sections.diagnostics), expected);
    })
}

#[test]
fn sections_merge_pruned_session_diagnostics_without_double_counting() {
    ccusage_test_support::install_pricing_snapshots();
    let good = r#"{"timestamp":"2026-01-02T00:00:00Z","message":{"id":"good","model":"claude-opus-5","usage":{"input_tokens":10,"output_tokens":1}}}"#;
    let bad = r#"{"timestamp":"2026-01-02T00:00:00Z","message":{"usage":{"input_tokens":"bad"}}}"#;
    let old_bad =
        r#"{"timestamp":"2001-01-01T00:00:00Z","message":{"usage":{"input_tokens":"bad"}}}"#;
    let fixture = fs_fixture!({
        "claude/projects/p/new.jsonl": [good, bad].join("\n"),
        "claude/projects/p/old.jsonl": [old_bad, old_bad].join("\n"),
    });
    set_modified(
        &fixture.path("claude/projects/p/old.jsonl"),
        instant("2001-01-01T00:00:00Z"),
    );
    isolated_agent_env(
        &fixture,
        "CLAUDE_CONFIG_DIR",
        fixture.path("claude").into_os_string(),
    )
    .run(|| {
        let shared = SharedArgs {
            mode: CostMode::Display,
            ..fixture_shared("20100101", "20991231")
        };

        for (since, expected) in [
            (
                Some("20100101".parse().unwrap()),
                "WARN  claude: skipped 1 record in 1 input (invalid data)",
            ),
            (
                None,
                "WARN  claude: skipped 3 records in 2 inputs (invalid data)",
            ),
        ] {
            let shared = SharedArgs {
                since,
                ..shared.clone()
            };
            let sections = loader::load_sections(
                &[AgentReportKind::Daily, AgentReportKind::Session],
                &shared,
                &NO_SOURCES,
                WeekDay::Monday,
            )
            .unwrap();

            let warnings = sections
                .diagnostics
                .iter()
                .flat_map(|diagnostics| diagnostics.warnings(&shared))
                .collect::<Vec<_>>();
            assert_eq!(warnings, [expected]);
        }
    })
}
