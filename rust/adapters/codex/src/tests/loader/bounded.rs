use super::*;

#[test]
fn bounded_loading_resolves_thread_id_rollout_parent_by_payload_id() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "sessions/2025/01/01/2025-01-01T08-00-00-thread_id_rollout.jsonl": [
            replay_metadata("2025-01-01T08:00:00.000Z", "parent-id", None),
            replay_token_count("2025-01-01T08:01:00.000Z", 100),
        ]
        .join("\n"),
        "sessions/2026/03/15/child.jsonl": [
            replay_metadata("2026-03-15T08:00:00.000Z", "child", Some("parent-id")),
            replay_token_count("2026-03-15T08:00:00.000Z", 100),
            replay_token_count("2026-03-15T08:01:00.000Z", 50),
        ]
        .join("\n"),
    });
    let parent = fixture.path("sessions/2025/01/01/2025-01-01T08-00-00-thread_id_rollout.jsonl");
    let child = fixture.path("sessions/2026/03/15/child.jsonl");
    crate::test_support::set_file_modified(
        &parent,
        crate::parse_ts_timestamp("2025-01-01T08:00:00.000Z").unwrap(),
    );
    crate::test_support::set_file_modified(
        &child,
        crate::parse_ts_timestamp("2026-03-15T08:00:00.000Z").unwrap(),
    );
    let sources = [CodexUsageSource::new(
        fixture.path("sessions"),
        fixture.root().to_path_buf(),
    )];
    let shared = SharedArgs {
        since: Some("20260315".parse().unwrap()),
        until: Some("20260315".parse().unwrap()),
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };

    for single_thread in [true, false] {
        let (sessions, detected) = session_input_tokens(
            &sources,
            &SharedArgs {
                single_thread,
                ..shared.clone()
            },
            &Diagnostics::new("codex"),
        );

        assert!(detected, "single_thread={single_thread}");
        assert_eq!(
            sessions,
            [("2026/03/15/child".to_string(), 50)],
            "single_thread={single_thread}"
        );
    }
}

#[test]
fn bounded_loading_does_not_select_a_misleading_filename() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "sessions/2025/01/01/aaa-target-id.jsonl": [
            replay_metadata("2025-01-01T08:00:00.000Z", "different-id", None),
            replay_token_count("2025-01-01T08:01:00.000Z", 999),
        ]
        .join("\n"),
        "sessions/2025/01/02/zzz-actual-parent.jsonl": [
            replay_metadata("2025-01-01T08:00:00.000Z", "target-id", None),
            replay_token_count("2025-01-01T08:01:00.000Z", 100),
        ]
        .join("\n"),
        "sessions/2026/03/15/child.jsonl": [
            replay_metadata("2026-03-15T08:00:00.000Z", "child", Some("target-id")),
            replay_token_count("2026-03-15T08:00:00.000Z", 100),
            replay_token_count("2026-03-15T08:01:00.000Z", 50),
        ]
        .join("\n"),
    });
    let misleading = fixture.path("sessions/2025/01/01/aaa-target-id.jsonl");
    let parent = fixture.path("sessions/2025/01/02/zzz-actual-parent.jsonl");
    let child = fixture.path("sessions/2026/03/15/child.jsonl");
    for path in [&misleading, &parent] {
        crate::test_support::set_file_modified(
            path,
            crate::parse_ts_timestamp("2025-01-01T08:00:00.000Z").unwrap(),
        );
    }
    crate::test_support::set_file_modified(
        &child,
        crate::parse_ts_timestamp("2026-03-15T08:00:00.000Z").unwrap(),
    );
    let sources = [CodexUsageSource::new(
        fixture.path("sessions"),
        fixture.root().to_path_buf(),
    )];
    let shared = SharedArgs {
        since: Some("20260315".parse().unwrap()),
        until: Some("20260315".parse().unwrap()),
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };

    for single_thread in [true, false] {
        let (sessions, detected) = session_input_tokens(
            &sources,
            &SharedArgs {
                single_thread,
                ..shared.clone()
            },
            &Diagnostics::new("codex"),
        );

        assert!(detected, "single_thread={single_thread}");
        assert_eq!(
            sessions,
            [("2026/03/15/child".to_string(), 50)],
            "single_thread={single_thread}"
        );
    }
}

#[test]
fn bounded_loading_uses_the_first_duplicate_parent_id() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "sessions/2025/01/01/aaa-first-parent.jsonl": [
            replay_metadata("2025-01-01T08:00:00.000Z", "parent-id", None),
            replay_token_count("2025-01-01T08:01:00.000Z", 100),
        ]
        .join("\n"),
        "sessions/2025/01/02/zzz-second-parent.jsonl": [
            replay_metadata("2025-01-01T08:00:00.000Z", "parent-id", None),
            replay_token_count("2025-01-01T08:01:00.000Z", 200),
        ]
        .join("\n"),
        "sessions/2026/03/15/child.jsonl": [
            replay_metadata("2026-03-15T08:00:00.000Z", "child", Some("parent-id")),
            replay_token_count("2026-03-15T08:00:00.000Z", 100),
            replay_token_count("2026-03-15T08:01:00.000Z", 50),
        ]
        .join("\n"),
    });
    let first_parent = fixture.path("sessions/2025/01/01/aaa-first-parent.jsonl");
    let second_parent = fixture.path("sessions/2025/01/02/zzz-second-parent.jsonl");
    let child = fixture.path("sessions/2026/03/15/child.jsonl");
    for path in [&first_parent, &second_parent] {
        crate::test_support::set_file_modified(
            path,
            crate::parse_ts_timestamp("2025-01-01T08:00:00.000Z").unwrap(),
        );
    }
    crate::test_support::set_file_modified(
        &child,
        crate::parse_ts_timestamp("2026-03-15T08:00:00.000Z").unwrap(),
    );
    let sources = [CodexUsageSource::new(
        fixture.path("sessions"),
        fixture.root().to_path_buf(),
    )];
    let shared = SharedArgs {
        since: Some("20260315".parse().unwrap()),
        until: Some("20260315".parse().unwrap()),
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };

    for single_thread in [true, false] {
        let (sessions, detected) = session_input_tokens(
            &sources,
            &SharedArgs {
                single_thread,
                ..shared.clone()
            },
            &Diagnostics::new("codex"),
        );

        assert!(detected, "single_thread={single_thread}");
        assert_eq!(
            sessions,
            [("2026/03/15/child".to_string(), 50)],
            "single_thread={single_thread}"
        );
    }
}

#[test]
fn bounded_loading_skips_a_self_parent_candidate_for_a_duplicate_parent_id() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "sessions/2025/01/01/aaa-child.jsonl": [
            replay_metadata(
                "2026-03-15T08:00:00.000Z",
                "shared-id",
                Some("shared-id"),
            ),
            replay_token_count("2026-03-15T08:00:01.000Z", 100),
            replay_token_count("2026-03-15T08:01:00.000Z", 50),
        ]
        .join("\n"),
        "sessions/2025/01/02/zzz-parent.jsonl": [
            replay_metadata("2025-01-01T08:00:00.000Z", "shared-id", None),
            replay_token_count("2025-01-01T08:01:00.000Z", 100),
        ]
        .join("\n"),
    });
    let child = fixture.path("sessions/2025/01/01/aaa-child.jsonl");
    let parent = fixture.path("sessions/2025/01/02/zzz-parent.jsonl");
    crate::test_support::set_file_modified(
        &child,
        crate::parse_ts_timestamp("2026-03-15T08:00:00.000Z").unwrap(),
    );
    crate::test_support::set_file_modified(
        &parent,
        crate::parse_ts_timestamp("2025-01-01T08:00:00.000Z").unwrap(),
    );
    let sources = [CodexUsageSource::new(
        fixture.path("sessions"),
        fixture.root().to_path_buf(),
    )];
    let shared = SharedArgs {
        since: Some("20260315".parse().unwrap()),
        until: Some("20260315".parse().unwrap()),
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    };

    for single_thread in [true, false] {
        let (sessions, detected) = session_input_tokens(
            &sources,
            &SharedArgs {
                single_thread,
                ..shared.clone()
            },
            &Diagnostics::new("codex"),
        );

        assert!(detected, "single_thread={single_thread}");
        assert_eq!(
            sessions,
            [("2025/01/01/aaa-child".to_string(), 50)],
            "single_thread={single_thread}"
        );
    }
}

#[test]
fn unbounded_loading_skips_a_self_parent_candidate_for_a_duplicate_parent_id() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "sessions/2025/01/01/aaa-child.jsonl": [
            replay_metadata(
                "2026-03-15T08:00:00.000Z",
                "shared-id",
                Some("shared-id"),
            ),
            replay_token_count("2026-03-15T08:00:01.000Z", 100),
            replay_token_count("2026-03-15T08:01:00.000Z", 50),
        ]
        .join("\n"),
        "sessions/2025/01/02/zzz-parent.jsonl": [
            replay_metadata("2025-01-01T08:00:00.000Z", "shared-id", None),
            replay_token_count("2025-01-01T08:01:00.000Z", 100),
        ]
        .join("\n"),
    });
    let child = fixture.path("sessions/2025/01/01/aaa-child.jsonl");
    let parent = fixture.path("sessions/2025/01/02/zzz-parent.jsonl");
    crate::test_support::set_file_modified(
        &child,
        crate::parse_ts_timestamp("2026-03-15T08:00:00.000Z").unwrap(),
    );
    crate::test_support::set_file_modified(
        &parent,
        crate::parse_ts_timestamp("2025-01-01T08:00:00.000Z").unwrap(),
    );

    for single_thread in [true, false] {
        let shared = SharedArgs {
            single_thread,
            ..SharedArgs::default()
        };
        let groups = load_groups_from_directory(
            &fixture.path("sessions"),
            &shared,
            AgentReportKind::Session,
        )
        .unwrap();

        assert_eq!(
            groups["2025/01/01/aaa-child"].input_tokens, 50,
            "single_thread={single_thread}"
        );
    }
}

#[test]
fn bounded_loading_metadata_probes_history_but_reads_parent_usage_only() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "sessions/2025/01/01/parent.jsonl": [
            replay_metadata("2025-01-01T08:00:00.000Z", "parent", None),
            replay_token_count("2025-01-01T08:01:00.000Z", 100),
        ]
        .join("\n"),
        "sessions/2025/01/01/unrelated.jsonl": [
            replay_metadata("2025-01-01T08:00:00.000Z", "unrelated", None),
            replay_token_count("2025-01-01T08:01:00.000Z", 999),
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"secret":"#.to_string(),
        ]
        .join("\n"),
        "sessions/2026/03/15/child.jsonl": [
            replay_metadata("2026-03-15T08:00:00.000Z", "child", Some("parent")),
            replay_token_count("2026-03-15T08:00:00.000Z", 100),
            replay_token_count("2026-03-15T08:01:00.000Z", 50),
        ]
        .join("\n"),
    });
    let parent = fixture.path("sessions/2025/01/01/parent.jsonl");
    let unrelated = fixture.path("sessions/2025/01/01/unrelated.jsonl");
    let child = fixture.path("sessions/2026/03/15/child.jsonl");
    for path in [&parent, &unrelated] {
        crate::test_support::set_file_modified(
            path,
            crate::parse_ts_timestamp("2025-01-01T08:00:00.000Z").unwrap(),
        );
    }
    crate::test_support::set_file_modified(
        &child,
        crate::parse_ts_timestamp("2026-03-15T08:00:00.000Z").unwrap(),
    );
    let sources = [CodexUsageSource::new(
        fixture.path("sessions"),
        fixture.root().to_path_buf(),
    )];
    let shared = SharedArgs {
        since: Some("20260315".parse().unwrap()),
        until: Some("20260315".parse().unwrap()),
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        single_thread: true,
        ..SharedArgs::default()
    };

    let diagnostics = Diagnostics::new("codex");
    let (sessions, detected) = session_input_tokens(&sources, &shared, &diagnostics);

    assert!(detected);
    assert_eq!(sessions, [("2026/03/15/child".to_string(), 50)]);
    assert_eq!(
        diagnostics.skipped(ccusage_core::SkipKind::InvalidData),
        0,
        "the unrelated history was read past its first line"
    );
}

#[test]
fn parent_replay_records_skips_once_however_often_the_parent_is_read() {
    let fixture = fs_fixture!({
        "sessions/2025/01/01/parent.jsonl": [
            replay_metadata("2025-01-01T08:00:00.000Z", "parent", None),
            replay_token_count("2025-01-01T08:01:00.000Z", 100),
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"secret":"#.to_string(),
            replay_token_count("2025-01-01T08:02:00.000Z", 120),
        ].join("\n") + "\n",
        "sessions/2025/01/02/child.jsonl": [
            replay_metadata("2025-01-02T08:00:00.000Z", "child", Some("parent")),
            replay_token_count("2025-01-02T08:00:00.000Z", 120),
        ].join("\n") + "\n",
    });
    let sessions = fixture.path("sessions");
    let parent = fixture.path("sessions/2025/01/01/parent.jsonl");
    let child = fixture.path("sessions/2025/01/02/child.jsonl");
    let files = vec![parent.clone(), child];
    let diagnostics = Diagnostics::new("codex");

    let plan_files = [(sessions.as_path(), files.as_slice())];
    let plan = CodexReplayPlan::for_selection(&plan_files, plan_files, true, &diagnostics);

    assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
    for file in &files {
        crate::parser::visit_codex_session_file_with_diagnostics(
            &sessions,
            file,
            plan.fork_replay(file),
            |_| {},
            &diagnostics,
        );
    }
    let _ = CodexReplayPlan::for_selection(&plan_files, plan_files, true, &diagnostics);
    assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
}

#[test]
fn the_fused_fork_pass_records_a_parent_skip_once() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "sessions/2025/01/01/parent.jsonl": [
            replay_metadata("2025-01-01T08:00:00.000Z", "parent", None),
            replay_token_count("2025-01-01T08:01:00.000Z", 100),
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"secret":"#.to_string(),
            replay_token_count("2025-01-01T08:02:00.000Z", 120),
        ].join("\n") + "\n",
        "sessions/2025/01/02/child.jsonl": [
            replay_metadata("2025-01-02T08:00:00.000Z", "child", Some("parent")),
            replay_token_count("2025-01-02T08:00:00.000Z", 120),
            replay_token_count("2025-01-02T08:01:00.000Z", 150),
        ].join("\n") + "\n",
    });
    let totals = [true, false].map(|single_thread| {
        let shared = SharedArgs {
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            single_thread,
            ..Default::default()
        };
        let diagnostics = Diagnostics::new("codex");
        let (groups, _) = crate::aggregate::load_groups_from_sources_with_diagnostics(
            &[CodexUsageSource::new(
                fixture.path("sessions"),
                fixture.path("sessions"),
            )],
            &shared,
            ccusage_core::cli::AgentReportKind::Daily,
            ccusage_core::cli::WeekDay::Monday,
            Some(&crate::PricingMap::load_embedded()),
            &diagnostics,
        )
        .unwrap();

        assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
        groups.values().map(|group| group.input_tokens).sum::<u64>()
    });
    assert_eq!(totals[0], totals[1]);
}

#[test]
fn bounded_parent_usage_failures_warn_once_without_changing_replay() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "sessions/2025/01/01/parent.jsonl": [
            replay_metadata("2025-01-01T08:00:00.000Z", "parent", None),
            replay_token_count("2025-01-01T08:01:00.000Z", 100),
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"secret":"SECRET_CANARY","#.to_string(),
        ].join("\n") + "\n",
        "sessions/2026/03/15/child.jsonl": [
            replay_metadata("2026-03-15T08:00:00.000Z", "child", Some("parent")),
            replay_token_count("2026-03-15T08:00:00.000Z", 100),
            replay_token_count("2026-03-15T08:01:00.000Z", 50),
        ].join("\n"),
    });
    let parent = fixture.path("sessions/2025/01/01/parent.jsonl");
    let child = fixture.path("sessions/2026/03/15/child.jsonl");
    crate::test_support::set_file_modified(
        &parent,
        crate::parse_ts_timestamp("2025-01-01T08:00:00.000Z").unwrap(),
    );
    crate::test_support::set_file_modified(
        &child,
        crate::parse_ts_timestamp("2026-03-15T08:00:00.000Z").unwrap(),
    );
    let sources = [CodexUsageSource::new(
        fixture.path("sessions"),
        fixture.root().to_path_buf(),
    )];

    for (since, parent_tokens) in [("20260315", 0), ("20250101", 100)] {
        let shared = SharedArgs {
            since: Some(since.parse().unwrap()),
            until: Some("20260315".parse().unwrap()),
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            single_thread: true,
            debug: true,
            ..Default::default()
        };
        let diagnostics = Diagnostics::new("codex");
        let (sessions, _) = session_input_tokens(&sources, &shared, &diagnostics);

        let expected = [
            ("2025/01/01/parent", parent_tokens),
            ("2026/03/15/child", 50),
        ]
        .into_iter()
        .filter(|(_, tokens)| *tokens > 0)
        .map(|(session, tokens)| (session.to_string(), tokens))
        .collect::<Vec<_>>();
        assert_eq!(sessions, expected);
        assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
        assert!(
            !diagnostics
                .warnings(&shared)
                .join("\n")
                .contains("SECRET_CANARY")
        );

        let diagnostics = Diagnostics::new("codex");
        let (groups, _) = crate::aggregate::load_groups_from_sources_with_diagnostics(
            &[CodexUsageSource::new(
                fixture.path("sessions"),
                fixture.path("sessions"),
            )],
            &shared,
            ccusage_core::cli::AgentReportKind::Daily,
            ccusage_core::cli::WeekDay::Monday,
            Some(&crate::PricingMap::load_embedded()),
            &diagnostics,
        )
        .unwrap();
        assert_eq!(
            groups.values().map(|group| group.input_tokens).sum::<u64>(),
            50 + parent_tokens
        );
        assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::InvalidData), 1);
    }
}
