#![cfg(test)]

use std::process::Output;

use ccusage_test_support::{Fixture, fs_fixture};
use serde_json::{Value, json};

mod support;

const SESSION_UUID: &str = "01a0bb8c-c7c0-7630-9d82-860b875930f0";
const SESSION_ID: &str =
    "2026/09/19/rollout-2026-09-19T23-22-40-01a0bb8c-c7c0-7630-9d82-860b875930f0";
const SESSION_FILE: &str = "rollout-2026-09-19T23-22-40-01a0bb8c-c7c0-7630-9d82-860b875930f0";
const OTHER_UUID: &str = "01a0cc9d-d8d1-8741-ae93-971c986041a1";

#[test]
fn codex_statusline_keeps_valid_usage_when_a_replacement_rollout_is_corrupt() {
    let fixture = codex_fixture();
    fixture.write_file(
        format!("codex/sessions/rollout-2026-09-20T12-00-00-{SESSION_UUID}_{OTHER_UUID}.jsonl.zst"),
        "corrupt-rollout-canary",
    );
    let output = run_cli_output(
        &fixture,
        &[
            "codex",
            "statusline",
            "--id",
            SESSION_UUID,
            "--json",
            "--debug",
        ],
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "{stderr}");
    let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["totalTokens"], 150);
    assert!(stderr.contains("codex: read interrupted:"), "{stderr}");
    assert!(!stderr.contains("corrupt-rollout-canary"), "{stderr}");
}

#[test]
fn codex_statusline_reports_today_without_reading_stdin() {
    let fixture = fs_fixture!({
        "codex/sessions/today.jsonl": usage("2026-11-19T12:00:00Z", 1_000, 50),
        "codex/sessions/old.jsonl": usage("2026-01-01T12:00:00Z", 9_000, 500),
    });

    let output = codex_command(support::hermetic_command(fixture.root()), &fixture)
        .args(["codex", "statusline"])
        .env("CCUSAGE_TEST_NOW", "2026-11-19T12:30:00Z")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"Codex | Today: 1,050 tokens | Est. $0.00\n");
}

#[test]
fn codex_statusline_filters_sessions_and_uses_shared_pricing() {
    let fixture = codex_fixture();
    let config = fixture.write_file(
        "ccusage.json",
        r#"{
            "codex": { "commands": { "statusline": {
                "pricingOverrides": { "gpt-5": {
                    "inputCostPerToken": 0.01,
                    "outputCostPerToken": 0.02,
                    "cacheReadInputTokenCost": 0.005
                } }
            } } }
        }"#,
    );
    let output = run_cli(
        &fixture,
        &[
            "codex",
            "statusline",
            "--id",
            SESSION_UUID,
            "--config",
            config.to_str().unwrap(),
        ],
    );

    assert_eq!(output, "Codex | Session: 150 tokens | Est. $1.95\n");

    let summary: Value = serde_json::from_str(&run_cli(
        &fixture,
        &[
            "codex",
            "statusline",
            "--id",
            SESSION_UUID,
            "--json",
            "--config",
            config.to_str().unwrap(),
        ],
    ))
    .unwrap();
    assert_eq!(summary["scope"], "session");
    assert_eq!(summary["sessionId"], SESSION_ID);
    assert_eq!(summary["totalTokens"], 150);
    assert_eq!(summary["costUSD"], 1.95);
}

#[test]
fn codex_statusline_thread_uuid_includes_replacement_rollout_spend() {
    let fixture = codex_fixture();
    fixture.write_file(
        format!("codex/sessions/{SESSION_ID}.jsonl"),
        [
            usage("2026-09-19T23:22:40Z", 100, 50),
            compaction("2026-09-19T23:23:00Z"),
        ]
        .join("\n"),
    );
    fixture.write_file(
        format!("codex/sessions/{SESSION_UUID}.jsonl"),
        [
            json!({"type":"session_meta","payload":{"id":"01a0cc9d-d8d1-8741-ae93-971c986041a1"}})
                .to_string(),
            usage("2026-09-20T13:00:00Z", 50, 0),
            compaction("2026-09-19T23:23:00Z"),
        ]
        .join("\n"),
    );
    let replacement = format!(
        "codex/sessions/2026/09/20/rollout-2026-09-20T12-00-00-{SESSION_UUID}_01a0ddae-e9e2-7852-bfa4-a82da97152b2.jsonl"
    );
    fixture.write_file(replacement, [
        json!({"timestamp":"2026-09-20T12:00:00Z","type":"session_meta","payload":{"id":SESSION_UUID}}).to_string(),
        usage("2026-09-20T12:00:01Z", 200, 50),
    ].join("\n"));
    let summary: Value = serde_json::from_str(&run_cli(
        &fixture,
        &["codex", "statusline", "--id", SESSION_UUID, "--json"],
    ))
    .unwrap();
    assert_eq!(summary["scope"], "session");
    assert_eq!(summary["totalTokens"], 440);
    assert_eq!(summary["sessionId"], SESSION_UUID);
    let physical: Value = serde_json::from_str(&run_cli(
        &fixture,
        &["codex", "statusline", "--id", SESSION_ID, "--json"],
    ))
    .unwrap();
    assert_eq!(physical["totalTokens"], 190);
}

#[test]
fn codex_statusline_session_includes_usage_outside_date_filters() {
    let fixture = codex_fixture();
    fixture.write_file(
        format!("codex/sessions/{SESSION_ID}.jsonl"),
        [
            usage("2026-09-19T23:22:40Z", 100, 50),
            usage("2026-09-20T00:10:00Z", 200, 50),
        ]
        .join("\n"),
    );
    let summary: Value = serde_json::from_str(&run_cli(
        &fixture,
        &[
            "codex",
            "statusline",
            "--id",
            SESSION_UUID,
            "--since",
            "20260920",
            "--until",
            "20260920",
            "--json",
        ],
    ))
    .unwrap();

    assert_eq!(summary["scope"], "session");
    assert_eq!(summary["totalTokens"], 400);
}

#[test]
fn codex_statusline_keeps_directory_dates_from_ahead_timezones() {
    let fixture = fs_fixture!({ "codex/sessions/.keep": "" });
    fixture.write_file(
        format!("codex/sessions/2026/09/20/rollout-2026-09-20T00-00-00-{SESSION_UUID}.jsonl"),
        [
            usage("2026-09-19T18:00:00Z", 100, 50),
            usage("2026-09-19T21:00:00Z", 200, 50),
        ]
        .join("\n"),
    );
    let summary: Value = serde_json::from_str(&run_cli(
        &fixture,
        &[
            "codex",
            "statusline",
            "--since",
            "20260919",
            "--until",
            "20260919",
            "--timezone",
            "Asia/Dubai",
            "--json",
        ],
    ))
    .unwrap();
    assert_eq!(summary["totalTokens"], 150);
}

#[test]
fn codex_statusline_skips_directories_after_the_last_possible_local_date() {
    let fixture = codex_fixture();
    fixture.write_file(
        "codex/sessions/2026/09/21/future.jsonl",
        usage("invalid", 100, 50),
    );
    let summary: Value = serde_json::from_str(&run_cli(
        &fixture,
        &[
            "codex",
            "statusline",
            "--since",
            "20260919",
            "--until",
            "20260919",
            "--json",
        ],
    ))
    .unwrap();
    assert_eq!(summary["totalTokens"], 150);
}

#[test]
fn codex_reports_keep_the_original_of_a_known_fork_compaction() {
    let fixture = fs_fixture!({
        "codex/sessions/2026/09/20/active.jsonl": [
            json!({"timestamp":"2026-09-19T21:00:00Z","type":"session_meta","payload":{"id":"child","forked_from_id":"parent"}}).to_string(),
            compaction("2026-09-19T21:00:00Z"),
        ].join("\n"),
        "codex/archived_sessions/2026/09/19/original.jsonl": [
            json!({"timestamp":"2026-09-19T18:00:00Z","type":"session_meta","payload":{"id":"parent"}}).to_string(),
            compaction("2026-09-19T18:00:00Z"),
        ].join("\n"),
    });
    for command in ["statusline", "daily", "session"] {
        for extra in [vec![], vec!["--single-thread"]] {
            let mut args = vec![
                "codex",
                command,
                "--since",
                "20260919",
                "--until",
                "20260919",
                "--timezone",
                "Asia/Dubai",
                "--json",
            ];
            args.extend(extra);
            let summary: Value = serde_json::from_str(&run_cli(&fixture, &args)).unwrap();
            let totals = summary.get("totals").unwrap_or(&summary);
            assert_eq!(totals["totalTokens"], 40, "{command}: {args:?}");
        }
    }
}

#[test]
fn codex_reports_keep_compaction_attribution_consistent_across_date_bounds() {
    let fixture = fs_fixture!({
        "codex/sessions/2026/09/20/active.jsonl": compaction("2026-09-19T21:00:00Z"),
        "codex/archived_sessions/2026/09/19/copy.jsonl": compaction("2026-09-19T18:00:00Z"),
    });
    for command in ["statusline", "daily", "session"] {
        for extra in [vec![], vec!["--single-thread"]] {
            let tokens = |since: &str, until: &str| {
                let mut args = vec![
                    "codex",
                    command,
                    "--since",
                    since,
                    "--until",
                    until,
                    "--timezone",
                    "Asia/Dubai",
                    "--json",
                ];
                args.extend_from_slice(&extra);
                let summary: Value = serde_json::from_str(&run_cli(&fixture, &args)).unwrap();
                summary.get("totals").unwrap_or(&summary)["totalTokens"]
                    .as_u64()
                    .unwrap()
            };
            let full = tokens("20260919", "20260920");
            let days = [
                tokens("20260919", "20260919"),
                tokens("20260920", "20260920"),
            ];
            assert_eq!(full, 40, "{command}: {extra:?}");
            assert_eq!(days, [0, 40], "{command}: {extra:?}");
            assert_eq!(days.iter().sum::<u64>(), full);
        }
    }
}

#[test]
fn codex_statusline_display_mode_matches_json_pricing_completeness() {
    let fixture = fs_fixture!({
        "codex/sessions/unknown.jsonl": usage("2026-09-19T12:00:00Z", 100, 50)
            .replace("gpt-5", "unknown-statusline-model"),
    });
    let args = [
        "codex",
        "statusline",
        "--since",
        "20260919",
        "--mode",
        "display",
    ];
    assert!(!run_cli(&fixture, &args).contains("incomplete pricing"));
    let summary: Value =
        serde_json::from_str(&run_cli(&fixture, &[&args[..], &["--json"]].concat())).unwrap();
    assert!(summary.get("unpricedModels").is_none());
}

#[test]
fn codex_statusline_date_range_matches_daily_totals() {
    let fixture = codex_fixture();
    let filters = ["--since", "2026-09-19", "--until", "20260919"];
    let summary: Value = serde_json::from_str(&run_cli(
        &fixture,
        &[&["codex", "statusline", "--json"][..], &filters].concat(),
    ))
    .unwrap();
    let daily: Value = serde_json::from_str(&run_cli(
        &fixture,
        &[&["codex", "daily", "--json"][..], &filters].concat(),
    ))
    .unwrap();

    assert_eq!(summary["scope"], "range");
    for (field, value) in daily["totals"].as_object().unwrap() {
        assert_eq!(&summary[field], value, "{field}");
    }
    assert_eq!(summary["totalTokens"], 150);
    assert_eq!(
        run_cli(
            &fixture,
            &["codex", "statusline", "--id", SESSION_UUID, "--no-cost"]
        ),
        "Codex | Session: 150 tokens\n"
    );
}

#[test]
fn codex_statusline_recovers_discovered_config_and_keeps_valid_settings() {
    let fixture = fs_fixture!({
        "cwd/.ccusage/ccusage.json": r#"{
            "defaults":{"since":"20261119","until":"20261119","timezone":"Pacific/Kiritimati"},
            "codex":{"commands":{"statusline":{
                "json":true, "timezone":"Invalid/Zone",
                "since":"20990101", "until":"20261119"
            }}}
        }"#,
        "codex/sessions/previous-utc-day.jsonl": usage("2026-11-18T12:00:00Z", 100, 50),
        "codex/sessions/outside-range.jsonl": usage("2026-11-17T12:00:00Z", 9000, 500),
    });
    let output = support::hermetic_command(fixture.root())
        .args(["--offline", "codex", "statusline"])
        .env("CODEX_HOME", fixture.path("codex"))
        .env("CCUSAGE_TEST_NOW", "2026-11-19T12:30:00Z")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(stderr.contains("Invalid ccusage config field:"));
    assert!(stderr.contains("ccusage.json: codex.commands.statusline.timezone:"));
    assert!(stderr.contains("ccusage.json: codex.commands.statusline.since:"));
    assert!(stderr.contains("Invalid/Zone"));
    assert!(stderr.contains("later than"));
    let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["scope"], "range");
    assert_eq!(summary["totalTokens"], 150);
}

#[test]
fn codex_statusline_recovers_reversed_discovered_windows_as_a_pair() {
    for (config, until_path, expected_scope, expected_tokens) in [
        (
            r#"{
                "codex":{"commands":{"statusline":{"since":"20261118","until":"20000101"}}},
                "defaults":{"since":"19990101","until":"20261118"}
            }"#,
            "codex.commands.statusline.until",
            "range",
            300,
        ),
        (
            r#"{"codex":{"defaults":{"until":"20000101"},"commands":{"statusline":{"since":"20261118"}}}}"#,
            "codex.defaults.until",
            "today",
            150,
        ),
    ] {
        let fixture = fs_fixture!({
            "cwd/.ccusage/ccusage.json": config,
            "codex/sessions/today.jsonl": usage("2026-11-18T12:00:00Z", 100, 50),
            "codex/sessions/previous-day.jsonl": usage("2026-11-17T12:00:00Z", 100, 50),
        });
        let output = support::hermetic_command(fixture.root())
            .args(["codex", "statusline", "--json", "--timezone", "UTC"])
            .env("CODEX_HOME", fixture.path("codex"))
            .env("CCUSAGE_TEST_NOW", "2026-11-18T13:00:00Z")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stderr}");
        assert!(
            stderr.lines().any(|line| {
                line.contains("codex.commands.statusline.since")
                    && line.contains(until_path)
                    && line.contains("later than")
            }),
            "{stderr}"
        );
        let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(summary["scope"], expected_scope);
        assert_eq!(summary["totalTokens"], expected_tokens);
    }
}

#[test]
fn codex_statusline_names_the_config_field_before_the_paired_flag() {
    for (field, config_bound, flag) in [
        ("until", "20000101", "--since"),
        ("since", "20990101", "--until"),
    ] {
        let fixture = fs_fixture!({
            "codex/sessions/today.jsonl": usage("2026-11-18T12:00:00Z", 100, 50),
        });
        let path = fixture.write_file(
            "cwd/.ccusage/ccusage.json",
            json!({"codex":{"commands":{"statusline":{field:config_bound}}}}).to_string(),
        );
        let path = path.canonicalize().unwrap();
        let output = support::hermetic_command(fixture.root())
            .args(["codex", "statusline", flag, "20261118", "--no-cost"])
            .env("CODEX_HOME", fixture.path("codex"))
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stderr}");
        let prefix = format!(
            "WARN Invalid ccusage config field: {}: codex.commands.statusline.{field}: ",
            path.display()
        );
        assert!(
            stderr.lines().any(|line| {
                line.starts_with(&prefix) && line.ends_with(&format!(" (paired with {flag})"))
            }),
            "{stderr}"
        );
        assert_eq!(output.stdout, b"Codex | Usage: 150 tokens\n");
    }
}

#[test]
fn invalid_discovered_statusline_bounds_warn_in_every_mode() {
    for (config, field, daily_exit) in [
        (
            json!({"defaults":{"since":"invalid-date"}}),
            "defaults.since",
            2,
        ),
        (
            json!({"codex":{"commands":{"statusline":{"until":"invalid-date"}}}}),
            "codex.commands.statusline.until",
            0,
        ),
    ] {
        let fixture = codex_fixture();
        let path = fixture.write_file("cwd/.ccusage/ccusage.json", config.to_string());
        let run = |args: &[&str]| {
            support::hermetic_command(fixture.root())
                .args(args)
                .env("CODEX_HOME", fixture.path("codex"))
                .env("CCUSAGE_TEST_NOW", "2026-09-19T23:30:00Z")
                .output()
                .unwrap()
        };
        let warning = format!(
            "{}: Invalid ccusage config field: {field}: not a valid date",
            path.display()
        );
        for (args, expected) in [
            (
                &["codex", "statusline", "--no-cost"][..],
                "Codex | Today: 150 tokens\n",
            ),
            (
                &["codex", "statusline", "--id", SESSION_UUID, "--no-cost"],
                "Codex | Session: 150 tokens\n",
            ),
            (&["codex", "statusline", "--mcp"], ""),
        ] {
            let output = run(args);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{stderr}");
            assert!(stderr.contains(&warning), "{stderr}");
            assert_eq!(output.stdout, expected.as_bytes());
        }

        let output = run(&["codex", "daily", "--offline", "--json"]);
        assert_eq!(output.status.code(), Some(daily_exit));
    }
}

#[test]
fn invalid_explicit_statusline_bounds_are_errors_in_every_mode() {
    let fixture = fs_fixture!({
        "bad-config.json": r#"{"defaults":{"since":"invalid-date"}}"#,
    });
    let path = fixture.path("bad-config.json");
    let config_error = format!("{}: defaults.since: not a valid date", path.display());
    for mode in [&[][..], &["--id", SESSION_UUID], &["--mcp"]] {
        for (bound, expected_error) in [
            (
                ["--since", "invalid-date"],
                "Invalid value for --since 'invalid-date'",
            ),
            (
                ["--until", "invalid-date"],
                "Invalid value for --until 'invalid-date'",
            ),
            (["--config", path.to_str().unwrap()], config_error.as_str()),
        ] {
            let output = run_cli_output(
                &fixture,
                &["codex", "statusline"]
                    .into_iter()
                    .chain(mode.iter().copied())
                    .chain(bound)
                    .collect::<Vec<_>>(),
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(output.status.code(), Some(2), "{stderr}");
            assert!(stderr.contains(expected_error), "{stderr}");
            assert!(output.stdout.is_empty());
        }
    }
}

#[test]
fn codex_statusline_uses_the_selected_timezone_for_today() {
    let fixture = fs_fixture!({
        "codex/sessions/today.jsonl": usage("2026-11-19T00:30:00Z", 100, 50),
        "codex/sessions/tomorrow.jsonl": usage("2026-11-19T23:30:00Z", 9_000, 500),
    });
    let output = support::hermetic_command(fixture.root())
        .args([
            "codex",
            "statusline",
            "--timezone",
            "Asia/Tokyo",
            "--no-cost",
        ])
        .env("CODEX_HOME", fixture.path("codex"))
        .env("CCUSAGE_TEST_NOW", "2026-11-19T12:00:00Z")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "Codex | Today: 150 tokens\n"
    );
}

#[test]
fn codex_statusline_handles_missing_usage_and_unknown_sessions() {
    let fixture = fs_fixture!({ "codex/sessions/.keep": "" });
    assert_eq!(
        run_cli(&fixture, &["codex", "statusline"]),
        "Codex | Today: 0 tokens | Est. $0.00\n"
    );
    let output = run_cli_output(&fixture, &["codex", "statusline", "--id", "missing"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("No Codex session found")
    );
}

#[test]
fn codex_statusline_marks_incomplete_pricing_and_can_hide_costs() {
    let fixture = fs_fixture!({
        "codex/sessions/unknown.jsonl": usage("2026-09-19T12:00:00.000Z", 100, 50)
            .replace("gpt-5", "unknown-statusline-model"),
    });
    let args = [
        "codex",
        "statusline",
        "--since",
        "20260919",
        "--until",
        "20260919",
    ];
    assert!(run_cli(&fixture, &args).contains("(incomplete pricing)"));
    let summary: Value =
        serde_json::from_str(&run_cli(&fixture, &[&args[..], &["--json"]].concat())).unwrap();
    assert_eq!(
        summary["unpricedModels"],
        json!(["unknown-statusline-model"])
    );

    let summary: Value = serde_json::from_str(&run_cli(
        &fixture,
        &[&args[..], &["--json", "--no-cost"]].concat(),
    ))
    .unwrap();
    assert_eq!(summary["totalTokens"], 150);
    assert!(summary.get("costUSD").is_none());
    assert_eq!(
        summary["unpricedModels"],
        json!(["unknown-statusline-model"])
    );
    let output = codex_command(
        support::hermetic_command_with_tools(fixture.root(), &["jq"]),
        &fixture,
    )
    .args(args)
    .args(["--jq", ".totalTokens"])
    .output()
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"150\n");
}

#[test]
fn selects_codex_session_by_supported_id_forms() {
    let fixture = codex_fixture();

    for requested_id in [
        SESSION_ID.to_string(),
        SESSION_FILE.to_string(),
        format!("{SESSION_FILE}.jsonl"),
        SESSION_UUID.to_string(),
        format!("codex://threads/{SESSION_UUID}"),
    ] {
        let output = run_cli(
            &fixture,
            &["codex", "session", "--id", &requested_id, "--json"],
        );
        let report: Value = serde_json::from_str(&output).unwrap();

        assert_eq!(report["sessionId"], SESSION_ID);
        assert_eq!(report["totalTokens"], 150);
        assert!(report.get("totalCost").is_some());
        assert!(report.get("costUSD").is_none());
        assert!(report.get("sessions").is_none());
        assert!(report.get("totals").is_none());
    }
}

#[test]
fn filters_codex_session_table_by_full_id() {
    let fixture = codex_fixture();

    let output = run_cli(
        &fixture,
        &["codex", "session", "--id", SESSION_ID, "--no-cost"],
    );

    assert!(output.contains(SESSION_UUID));
    assert!(!output.contains(OTHER_UUID));
    let selected_row = output
        .lines()
        .find(|line| line.contains(SESSION_UUID))
        .expect("selected session row should render");
    assert!(selected_row.contains("150"));
}

#[test]
fn reports_unknown_codex_session_id() {
    let fixture = codex_fixture();

    let output = run_cli_output(&fixture, &["codex", "session", "--id", "missing"]);

    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("No Codex session found with ID: missing")
    );
}

#[test]
fn reports_ambiguous_codex_session_uuid() {
    let fixture = fs_fixture!({
        "codex/sessions/2026/09/19/rollout-2026-09-19T23-22-40-01a0bb8c-c7c0-7630-9d82-860b875930f0.jsonl": usage("2026-09-19T23:22:40.000Z", 100, 50),
        "codex/sessions/2026/09/20/rollout-2026-09-20T10-00-00-01a0bb8c-c7c0-7630-9d82-860b875930f0.jsonl": usage("2026-09-20T10:00:00.000Z", 200, 75),
    });

    let output = run_cli_output(&fixture, &["codex", "session", "--id", SESSION_UUID]);

    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr).unwrap().contains(&format!(
        "Codex session ID '{SESSION_UUID}' is ambiguous and matches 2 sessions."
    )));
}

#[test]
fn reports_compressed_codex_rollouts_alongside_plain_ones() {
    let fixture = codex_fixture();
    let compressed = fixture.write_file(
        format!("codex/archived_sessions/2026/09/21/{SESSION_FILE}-zst.jsonl.zst"),
        "",
    );
    std::fs::write(
        &compressed,
        zstd::stream::encode_all(usage("2026-09-21T08:00:00.000Z", 300, 25).as_bytes(), 3).unwrap(),
    )
    .unwrap();

    let daily: Value =
        serde_json::from_str(&run_cli(&fixture, &["codex", "daily", "--json"])).unwrap();
    let session: Value = serde_json::from_str(&run_cli(
        &fixture,
        &[
            "codex",
            "session",
            "--id",
            &format!("{SESSION_FILE}-zst.jsonl.zst"),
            "--json",
        ],
    ))
    .unwrap();

    assert_eq!(daily["daily"].as_array().unwrap().len(), 3);
    assert_eq!(daily["totals"]["totalTokens"], 150 + 1000 + 325);
    assert_eq!(
        session["sessionId"],
        format!("2026/09/21/{SESSION_FILE}-zst")
    );
    assert_eq!(session["totalTokens"], 325);
}

#[test]
fn reports_issue_1970_subagent_usage_in_daily_and_session_commands() {
    let fixture = issue_1970_codex_fixture();

    let daily_json = run_cli(
        &fixture,
        &[
            "codex", "daily", "--since", "20260101", "--until", "20260131", "--json",
        ],
    );
    let daily: Value = serde_json::from_str(&daily_json).unwrap();
    assert_eq!(daily["totals"]["totalTokens"], 1010);
    let daily_row = daily["daily"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["date"] == "2026-01-05")
        .expect("issue 1970 daily row should exist");
    assert_eq!(daily_row["totalTokens"], 1010);

    let session_json = run_cli(
        &fixture,
        &[
            "codex", "session", "--since", "20260101", "--until", "20260131", "--json",
        ],
    );
    let session: Value = serde_json::from_str(&session_json).unwrap();
    assert_eq!(session["totals"]["totalTokens"], 1010);
    let session_row = session["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["sessionFile"] == "child")
        .expect("issue 1970 session row should exist");
    assert_eq!(session_row["totalTokens"], 1010);

    insta::assert_snapshot!(
        "issue_1970_daily_table",
        format!(
            "Daily\n{}",
            run_cli(
                &fixture,
                &[
                    "codex", "daily", "--since", "20260101", "--until", "20260131",
                ],
            ),
        )
    );
    insta::assert_snapshot!(
        "issue_1970_session_table",
        format!(
            "Session\n{}",
            run_cli(
                &fixture,
                &[
                    "codex", "session", "--since", "20260101", "--until", "20260131",
                ],
            ),
        )
    );
}

#[test]
fn prices_recorded_and_configured_ultrafast_usage() {
    let settings = |timestamp: &str, tier: &str| {
        json!({
            "timestamp": timestamp,
            "type": "event_msg",
            "payload": {
                "type": "thread_settings_applied",
                "thread_settings": { "service_tier": tier },
            },
        })
        .to_string()
    };
    let request = |timestamp: &str| {
        json!({
            "timestamp": timestamp,
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "model": "gpt-6.1-sol",
                    "last_token_usage": {
                        "input_tokens": 1200,
                        "cached_input_tokens": 200,
                        "output_tokens": 50,
                        "total_tokens": 1250,
                    },
                },
            },
        })
        .to_string()
    };
    let ultrafast_only = [
        settings("2026-10-27T10:00:00Z", "ultrafast"),
        request("2026-10-27T10:00:01Z"),
    ]
    .join("\n");
    let switched_back = [
        settings("2026-10-27T10:00:00Z", "ultrafast"),
        request("2026-10-27T10:00:01Z"),
        settings("2026-10-27T10:00:02Z", "default"),
        request("2026-10-27T10:00:03Z"),
    ]
    .join("\n");
    let configured_ultrafast = [
        request("2026-10-27T10:00:01Z"),
        settings("2026-10-27T10:00:02Z", "default"),
        request("2026-10-27T10:00:03Z"),
    ]
    .join("\n");

    for (configured_tier, rollout, total_tokens, expected_cost) in [
        ("default", ultrafast_only, 1250, 0.01512),
        ("default", switched_back, 2500, 0.01512 + 0.00252),
        ("ultrafast", configured_ultrafast, 2500, 0.01512 + 0.00252),
    ] {
        let fixture = fs_fixture!({
            "codex/config.toml": format!("service_tier = {configured_tier:?}"),
            "codex/sessions/2026/10/27/rollout.jsonl": rollout,
        });

        let daily: Value = serde_json::from_str(&run_cli(
            &fixture,
            &["codex", "daily", "--speed", "auto", "--json"],
        ))
        .unwrap();

        assert_eq!(daily["totals"]["totalTokens"], total_tokens);
        let cost = daily["totals"]["costUSD"].as_f64().unwrap();
        assert!(
            (cost - expected_cost).abs() < 1e-12,
            "config tier {configured_tier}: expected {expected_cost}, got {cost}"
        );
        let summary: Value = serde_json::from_str(&run_cli(
            &fixture,
            &[
                "codex",
                "statusline",
                "--since",
                "20261027",
                "--until",
                "20261027",
                "--json",
            ],
        ))
        .unwrap();
        assert_eq!(summary["costUSD"], daily["totals"]["costUSD"]);
        assert_eq!(summary["totalTokens"], total_tokens);
    }
}

#[test]
fn prices_a_cross_home_rollout_copy_at_its_configured_fast_tier() {
    assert_cross_home_rollout_copy_cost(None, "fast", "gpt-5.6-sol", 0.008);
}

#[test]
fn prices_cross_home_rollout_copies_at_the_highest_model_multiplier() {
    for (first, second, model, cost) in [
        ("standard", "fast", "gpt-5.6-sol", 0.008),
        ("default", "fast", "gpt-5.6-sol", 0.008),
        ("ultrafast", "fast", "gpt-5.6-sol", 0.008),
        ("ultrafast", "fast", "gpt-6.1-sol", 0.012),
        ("flex", "standard", "gpt-5.6-sol", 0.004),
    ] {
        assert_cross_home_rollout_copy_cost(Some(first), second, model, cost);
        assert_cross_home_rollout_copy_cost(Some(second), first, model, cost);
    }
}

fn assert_cross_home_rollout_copy_cost(first: Option<&str>, second: &str, model: &str, cost: f64) {
    let rollout = json!({
        "timestamp": "2026-10-24T10:00:00Z",
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": {
                "model": model,
                "last_token_usage": { "input_tokens": 1000, "total_tokens": 1000 },
            },
        },
    })
    .to_string();
    let fixture = fs_fixture!({
        "codex/sessions/copied.jsonl": &rollout,
        "other/archived_sessions/copied.jsonl": &rollout,
        "other/config.toml": format!("service_tier = {second:?}"),
    });
    if let Some(first) = first {
        fixture.write_file("codex/config.toml", format!("service_tier = {first:?}"));
    }
    let homes = format!(
        "{},{}",
        fixture.path("codex").display(),
        fixture.path("other").display()
    );
    let output = codex_command(support::hermetic_command(fixture.root()), &fixture)
        .env("CODEX_HOME", homes)
        .args(["codex", "daily", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["totals"]["totalTokens"], 1000);
    assert_eq!(
        report["totals"]["costUSD"], cost,
        "{model} with {first:?} and {second}"
    );
}

#[test]
fn prices_an_unsupported_ultrafast_home_at_standard_even_next_to_flex() {
    assert_codex_home_costs(
        ("ultrafast", "flex"),
        [("gpt-5.6-sol", 0.004), ("gpt-5.6-sol", 0.002)],
    );
}

#[test]
fn prices_an_unsupported_fast_home_at_standard_even_next_to_flex() {
    assert_codex_home_costs(
        ("fast", "flex"),
        [("gpt-5.5-pro", 0.030), ("gpt-5.5-pro", 0.015)],
    );
}

#[test]
fn prices_standard_and_fast_homes_independently() {
    assert_codex_home_costs(
        ("standard", "fast"),
        [("gpt-5.6-sol", 0.004), ("gpt-5.6-sol", 0.008)],
    );
}

#[test]
fn prices_fast_and_ultrafast_homes_independently() {
    assert_codex_home_costs(
        ("fast", "ultrafast"),
        [
            ("gpt-5.6-sol", 0.008),
            ("gpt-6.1-sol", 0.012),
            ("gpt-6.1-sol", 0.004),
            ("gpt-5.6-sol", 0.004),
        ],
    );
}

fn assert_codex_home_costs<const N: usize>(tiers: (&str, &str), requests: [(&str, f64); N]) {
    let fixture = fs_fixture!({
        "codex/config.toml": format!("service_tier = {:?}", tiers.0),
        "other/config.toml": format!("service_tier = {:?}", tiers.1),
    });
    for (index, (model, _)) in requests.iter().enumerate() {
        let sessions = ["codex/sessions", "other/archived_sessions"][index % 2];
        fixture.write_file(
            format!("{sessions}/request-{index}.jsonl"),
            json!({
                "timestamp": format!("2026-10-{:02}T10:00:00Z", 24 + index),
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "model": model,
                        "last_token_usage": { "input_tokens": 1000, "total_tokens": 1000 },
                    },
                },
            })
            .to_string(),
        );
    }
    let homes = format!(
        "{},{}",
        fixture.path("codex").display(),
        fixture.path("other").display()
    );
    let run = |args: &[&str]| -> Value {
        let output = codex_command(support::hermetic_command(fixture.root()), &fixture)
            .env("CODEX_HOME", &homes)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    };
    let report = run(&["codex", "daily", "--json"]);
    let days = report["daily"].as_array().unwrap();
    assert_eq!(days.len(), N);
    for (index, (day, (model, cost))) in days.iter().zip(requests).enumerate() {
        assert_eq!(day["totalTokens"], 1000);
        assert_eq!(day["costUSD"], cost, "{model} on {}", day["date"]);
        let session = format!("request-{index}");
        let statusline = run(&["codex", "statusline", "--id", &session, "--json"]);
        assert_eq!(statusline["totalTokens"], 1000);
        assert_eq!(statusline["costUSD"], cost, "{model} in {session}");
    }
}

#[test]
fn reports_issue_2046_identical_fork_instant_requests_in_every_report_mode() {
    let fixture = fs_fixture!({
        "codex/sessions/2026/11/01/rollout-2026-11-01T09-42-09-01a1967c-0000-7000-8000-00000000000a.jsonl":
            include_str!("fixtures/codex/issue_2046.jsonl"),
    });

    for (mode, rows) in [
        ("daily", "daily"),
        ("monthly", "monthly"),
        ("session", "sessions"),
    ] {
        let report: Value =
            serde_json::from_str(&run_cli(&fixture, &["codex", mode, "--json"])).unwrap();
        let rows = report[rows].as_array().unwrap();
        assert_eq!(rows.len(), 1, "{mode}");
        for usage in [&rows[0], &report["totals"]] {
            assert_eq!(usage["inputTokens"], 16_000, "{mode}");
            assert_eq!(usage["cacheReadTokens"], 4_000, "{mode}");
            assert_eq!(usage["outputTokens"], 200, "{mode}");
        }

        insta::assert_snapshot!(
            format!("issue_2046_{mode}_table"),
            run_cli(&fixture, &["codex", mode])
        );
    }
}

#[test]
fn reports_issue_2184_spawned_siblings_with_identical_usage_separately() {
    let spawned = include_str!("fixtures/codex/issue_2046.jsonl");
    let sibling = spawned
        .replace("00000000000a", "00000000000b")
        .replace("resp_child_1", "resp_child_3")
        .replace("resp_child_2", "resp_child_4");
    let fixture = fs_fixture!({
        "codex/sessions/spawned.jsonl": spawned,
        "codex/sessions/spawned-second.jsonl": sibling,
    });

    let report: Value = serde_json::from_str(&run_cli(
        &fixture,
        &[
            "codex", "daily", "--json", "--since", "20261101", "--until", "20261101",
        ],
    ))
    .unwrap();
    let totals = &report["totals"];
    assert_eq!(totals["inputTokens"], 32_000);
    assert_eq!(totals["cacheReadTokens"], 8_000);
    assert_eq!(totals["outputTokens"], 400);
    assert_eq!(totals["totalTokens"], 40_400);
}

#[test]
fn session_reports_credit_a_replaced_stand_in_once() {
    let timestamp = "2026-11-01T09:00:00Z";
    let fixture = fs_fixture!({
        "codex/sessions/child.jsonl": [
            usage(timestamp, 1_000, 10),
            usage("2026-11-01T09:01:00Z", 300, 3),
        ].join("\n"),
        "codex/sessions/parent.jsonl": recorded_request(timestamp),
    });

    let reports = [
        ("sessions", "sessionId", &["codex", "session"][..]),
        (
            "sessions",
            "sessionId",
            &[
                "codex", "session", "--since", "20261101", "--until", "20261101",
            ],
        ),
        ("session", "period", &["session"]),
        (
            "session",
            "period",
            &["session", "--since", "20261101", "--until", "20261101"],
        ),
    ];
    let actual = [&[][..], &["--single-thread"]].map(|threading| {
        reports.map(|(rows, session_id, report)| {
            let args = [report, &["--json"], threading].concat();
            let output: Value = serde_json::from_str(&run_cli(&fixture, &args)).unwrap();
            let mut session_tokens = output[rows]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| {
                    (
                        row[session_id].as_str().unwrap().to_owned(),
                        row["totalTokens"].as_u64().unwrap(),
                    )
                })
                .collect::<Vec<_>>();
            session_tokens.sort_unstable();
            (
                output["totals"]["totalTokens"].as_u64().unwrap(),
                session_tokens,
            )
        })
    });
    let expected = (
        1_313,
        vec![("child".to_owned(), 303), ("parent".to_owned(), 1_010)],
    );
    assert_eq!(
        actual,
        std::array::from_fn(|_| std::array::from_fn(|_| expected.clone())),
        "parallel then serial, for {reports:?}"
    );
}

#[test]
fn session_and_daily_reports_dedupe_copied_records_across_sessions() {
    let request = recorded_request("2026-11-01T09:00:00Z");
    let fixture = fs_fixture!({
        "codex/sessions/original.jsonl": &request,
        "codex/sessions/copy.jsonl": &request,
    });

    let totals = [&[][..], &["--single-thread"]].map(|threading| {
        ["session", "daily"].map(|report| {
            let args = [&["codex", report, "--json"][..], threading].concat();
            let output: Value = serde_json::from_str(&run_cli(&fixture, &args)).unwrap();
            output["totals"]["totalTokens"].as_u64().unwrap()
        })
    });
    assert_eq!(totals, [[1_010, 1_010]; 2], "parallel then serial");
}

fn recorded_request(timestamp: &str) -> String {
    [
        json!({
            "timestamp": timestamp,
            "type": "token_usage_record",
            "payload": {
                "response_id": "resp_parent",
                "usage": {
                    "input_tokens": 1_000,
                    "cached_input_tokens": 10,
                    "output_tokens": 10,
                    "total_tokens": 1_010,
                },
            },
        })
        .to_string(),
        usage(timestamp, 1_000, 10),
    ]
    .join("\n")
}

fn issue_1970_codex_fixture() -> Fixture {
    fs_fixture!({
        "codex/sessions/2026/01/05/child.jsonl":
            include_str!("fixtures/codex/issue_1970.jsonl"),
    })
}

fn codex_fixture() -> Fixture {
    fs_fixture!({
        "codex/sessions/2026/09/19/rollout-2026-09-19T23-22-40-01a0bb8c-c7c0-7630-9d82-860b875930f0.jsonl": usage("2026-09-19T23:22:40.000Z", 100, 50),
        "codex/sessions/2026/09/20/rollout-2026-09-20T10-00-00-01a0cc9d-d8d1-8741-ae93-971c986041a1.jsonl": usage("2026-09-20T10:00:00.000Z", 900, 100),
    })
}

fn compaction(timestamp: &str) -> String {
    [
        json!({"type":"turn_context","payload":{"model":"gpt-5"}}),
        json!({"timestamp":timestamp,"type":"token_usage_record","payload":{
            "response_id":"compaction-response","usage":{"input_tokens":40,"output_tokens":0}
        }}),
        json!({"type":"compacted","payload":{"compaction_response_id":"compaction-response"}}),
    ]
    .iter()
    .map(Value::to_string)
    .collect::<Vec<_>>()
    .join("\n")
}

fn usage(timestamp: &str, input_tokens: u64, output_tokens: u64) -> String {
    json!({
        "timestamp": timestamp,
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": {
                "model": "gpt-5",
                "last_token_usage": {
                    "input_tokens": input_tokens,
                    "cached_input_tokens": 10,
                    "output_tokens": output_tokens,
                    "reasoning_output_tokens": 0,
                    "total_tokens": input_tokens + output_tokens,
                },
            },
        },
    })
    .to_string()
}

fn run_cli(fixture: &Fixture, args: &[&str]) -> String {
    let output = run_cli_output(fixture, args);
    assert!(
        output.status.success(),
        "ccusage CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("ccusage CLI stdout should be UTF-8")
}

fn run_cli_output(fixture: &Fixture, args: &[&str]) -> Output {
    codex_command(support::hermetic_command(fixture.root()), fixture)
        .args(args)
        .output()
        .expect("ccusage CLI should run")
}

fn codex_command(mut command: std::process::Command, fixture: &Fixture) -> std::process::Command {
    command
        .args(["--offline", "--no-color", "--timezone", "UTC"])
        .env("CODEX_HOME", fixture.path("codex"))
        .env("LOG_LEVEL", "0")
        .env("COLUMNS", "240");
    command
}
