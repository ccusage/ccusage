#![cfg(test)]

//! Golden CLI snapshots for each usage source and unified report.

use std::collections::BTreeSet;

use ccusage_test_support::{Environment, Fixture};

mod golden;
mod support;

use golden::{ALL_REPORT_KINDS, AgentMatrix, Case, UNSUPPORTED_VARIANT, agent_cases, run_case};

#[test]
fn seed_environment_preserves_list_entries() {
    use std::{ffi::OsString, path::Path};

    fn first(_root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
        vec![(
            "CLAUDE_CONFIG_DIR",
            vec!["first".into(), " first ".into(), "".into()],
        )]
    }

    fn second(_root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
        vec![("CLAUDE_CONFIG_DIR", vec!["second".into()])]
    }

    let agents = [
        AgentMatrix {
            agent: "first",
            seed: first,
            extras: golden::no_extras,
        },
        AgentMatrix {
            agent: "second",
            seed: second,
            extras: golden::no_extras,
        },
    ];
    let fixture = Fixture::new();
    let seeded = golden::seed_all(&agents, fixture.root());
    let command = golden::hermetic_command(fixture.root(), &seeded, 200);

    assert_eq!(
        command
            .get_envs()
            .find(|(name, _)| *name == "CLAUDE_CONFIG_DIR")
            .unwrap()
            .1,
        Some(std::ffi::OsStr::new("first, first ,,second")),
    );
}

#[cfg(unix)]
#[test]
fn seed_environment_preserves_non_utf8_paths() {
    use std::{
        ffi::{OsStr, OsString},
        os::unix::ffi::{OsStrExt as _, OsStringExt as _},
    };

    let fixture = Fixture::new();
    let seeded = [(
        "CLAUDE_CONFIG_DIR",
        vec![OsString::from_vec(b"store-\xff".to_vec()), "other".into()],
    )];
    let command = golden::hermetic_command(fixture.root(), &seeded, 200);

    assert_eq!(
        command
            .get_envs()
            .find(|(name, _)| *name == "CLAUDE_CONFIG_DIR")
            .unwrap()
            .1,
        Some(OsStr::from_bytes(b"store-\xff,other")),
    );
}

macro_rules! agent_matrix {
    ($($agent:ident => $extras:expr),* $(,)?) => {
        const AGENTS: &[AgentMatrix] = &[
            $(AgentMatrix {
                agent: stringify!($agent),
                seed: golden::$agent::seed,
                extras: $extras,
            }),*
        ];

        mod agent {
            use super::golden;

            $(
                #[test]
                fn $agent() {
                    golden::run_matrix(stringify!($agent), golden::$agent::seed, $extras);
                }
            )*
        }
    };
}

agent_matrix! {
    claude => golden::claude::extra_cases,
    codex => golden::codex::extra_cases,
    opencode => golden::opencode::extra_cases,
    amp => golden::no_extras,
    droid => golden::no_extras,
    codebuff => golden::no_extras,
    hermes => golden::no_extras,
    pi => golden::pi::extra_cases,
    goose => golden::no_extras,
    kilo => golden::kilo::extra_cases,
    copilot => golden::no_extras,
    gemini => golden::no_extras,
    antigravity => golden::antigravity::extra_cases,
    kimi => golden::no_extras,
    qwen => golden::no_extras,
    openclaw => golden::openclaw::extra_cases,
    grok => golden::no_extras,
    zcode => golden::zcode::extra_cases,
    cline => golden::no_extras,
    dsh => golden::dsh::extra_cases,
}

#[test]
fn codebuff_fixture_keeps_distinct_messages_at_the_same_instant() {
    let fixture = Fixture::new();
    let seeded = golden::codebuff::seed(fixture.root());
    let messages = |store: &str| {
        let path = fixture.root().join(format!(
            "codebuff/{store}/manicode/projects/alpha/chats/2025-12-31T23-50-00.000Z/chat-messages.json"
        ));
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(path).unwrap()).unwrap()
    };
    let original = messages(".");
    let backup = messages("backup");
    assert_eq!(original[1]["timestamp"], backup[1]["timestamp"]);
    assert_ne!(original[1]["id"], backup[1]["id"]);
    for flags in [vec![], vec!["--single-thread"]] {
        let mut case = Case::new("daily", "", &["codebuff", "daily", "--offline", "--json"]);
        case.args.extend(flags.into_iter().map(str::to_string));
        let output = run_case(fixture.root(), &seeded, &case);
        assert_eq!(output.exit, "0", "{}", output.stderr);
        let report: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
        let days = report["daily"].as_array().unwrap();
        let day = |date: &str| days.iter().find(|day| day["date"] == date).unwrap();
        assert_eq!(day("2025-12-31")["credits"], 2.75);
        assert_eq!(day("2026-01-01")["inputTokens"], 1500);
        assert_eq!(day("2026-01-01")["totalCost"], 0.0084);
    }
}

#[test]
fn codebuff_timestamp_ties_pin_order_and_replay_winner() {
    let fixture = Fixture::new();
    let seeded = golden::codebuff::seed_timestamp_ties(fixture.root());
    let case = Case::new(
        "daily",
        "json_tie_order",
        &["codebuff", "daily", "--offline", "--json"],
    );
    let output = run_case(fixture.root(), &seeded, &case);
    assert_eq!(output.exit, "0", "{}", output.stderr);
    let report: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(
        report["daily"][0]["modelsUsed"],
        serde_json::json!(["claude-sonnet-4-20250514", "claude-haiku-4-5-20251001"])
    );
    assert_eq!(report["totals"]["inputTokens"], 30);
    assert_eq!(report["daily"][0]["credits"], 2.75);
    for flags in [vec![], vec!["--single-thread".to_string()]] {
        let mut case = Case::new(
            "daily",
            "json_tie_order",
            &["codebuff", "daily", "--offline", "--json"],
        );
        case.args.extend(flags);
        let actual = run_case(fixture.root(), &seeded, &case);
        assert_eq!(actual.stdout, output.stdout);
    }
    golden::assert_case("codebuff", fixture.root(), &seeded, &case);
}

#[test]
fn focused_week_start_controls_buckets_and_last_together() {
    for (agent, seed) in [
        ("cline", golden::cline::seed as golden::Seed),
        ("dsh", golden::dsh::seed),
        ("opencode", golden::opencode::seed),
    ] {
        let fixture = Fixture::new();
        let seeded = seed(fixture.root());
        let config = fixture.write_file(
            "week.json",
            r#"{"commands":{"weekly":{"startOfWeek":"wednesday"}}}"#,
        );
        let daily = Case::new(
            "daily",
            "",
            &[
                agent,
                "daily",
                "--offline",
                "--json",
                "--timezone",
                "UTC",
                "--since",
                "20251231",
            ],
        );
        let expected: serde_json::Value =
            serde_json::from_str(&run_case(fixture.root(), &seeded, &daily).stdout).unwrap();
        for flags in [
            vec!["-w".to_string(), "wednesday".to_string()],
            vec!["--config".to_string(), config.display().to_string()],
        ] {
            let mut weekly = Case::new(
                "weekly",
                "",
                &[
                    agent,
                    "weekly",
                    "--offline",
                    "--json",
                    "--timezone",
                    "UTC",
                    "--last",
                    "1",
                ],
            )
            .env("CCUSAGE_TEST_NOW", "2026-01-01T12:00:00Z");
            weekly.args.extend(flags);
            let output = run_case(fixture.root(), &seeded, &weekly);
            assert_eq!(output.exit, "0", "{}", output.stderr);
            let report: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
            assert_eq!(
                report["weekly"].as_array().unwrap().len(),
                1,
                "{agent}: {report}"
            );
            assert_eq!(report["weekly"][0]["week"], "2025-12-31", "{agent}");
            assert_eq!(
                report["totals"]["totalTokens"], expected["totals"]["totalTokens"],
                "{agent}"
            );
            assert_eq!(
                report["totals"]["totalCost"], expected["totals"]["totalCost"],
                "{agent}"
            );
        }
    }
}

#[test]
fn last_stops_at_the_end_of_the_current_period() {
    fn rows(
        root: &std::path::Path,
        seeded: &[(&'static str, Vec<std::ffi::OsString>)],
        agent: &'static str,
        kind: &'static str,
        now: &'static str,
    ) -> Vec<serde_json::Value> {
        let case = Case::new(
            kind,
            "",
            &[
                agent,
                kind,
                "--offline",
                "--json",
                "--timezone",
                "UTC",
                "--last",
                "1",
            ],
        )
        .env("CCUSAGE_TEST_NOW", now);
        let output = run_case(root, seeded, &case);
        assert_eq!(output.exit, "0", "{}", output.stderr);
        let report: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
        report[kind].as_array().unwrap().clone()
    }

    fn keys(rows: &[serde_json::Value], field: &str) -> Vec<String> {
        rows.iter()
            .map(|row| row[field].as_str().unwrap().to_string())
            .collect()
    }

    let codex_fixture = Fixture::new();
    let codex = golden::codex::seed(codex_fixture.root());
    let codex_daily = rows(
        codex_fixture.root(),
        &codex,
        "codex",
        "daily",
        "2025-12-31T12:00:00Z",
    );
    assert_eq!(
        keys(&codex_daily, "date"),
        ["2025-12-31"],
        "a daily --last 1 window ends today"
    );

    let opencode_fixture = Fixture::new();
    let root = opencode_fixture.root();
    let opencode = golden::opencode::seed(root);
    let last_week = rows(
        root,
        &opencode,
        "opencode",
        "weekly",
        "2025-12-28T12:00:00Z",
    );
    assert_eq!(
        keys(&last_week, "week"),
        ["2025-12-22"],
        "a weekly --last 1 window ends with the current week"
    );
    let current_week = rows(
        root,
        &opencode,
        "opencode",
        "weekly",
        "2025-12-31T12:00:00Z",
    );
    assert_eq!(
        keys(&current_week, "week"),
        ["2025-12-29"],
        "a weekly window keeps the later days of the current week"
    );
    assert_eq!(
        current_week[0]["totalTokens"], 27117,
        "the week counts every record through its last day, including 1-2 January"
    );
    let current_month = rows(
        root,
        &opencode,
        "opencode",
        "monthly",
        "2025-12-31T12:00:00Z",
    );
    assert_eq!(
        keys(&current_month, "month"),
        ["2025-12"],
        "a monthly --last 1 window ends with the current month"
    );
}

#[test]
fn pi_tied_fork_keeps_the_first_path_in_both_read_modes() {
    for parent_first in [true, false] {
        let fixture = Fixture::new();
        let root = fixture.root();
        let seeded = golden::pi::seed_tied_fork(root, parent_first);
        for flags in [vec![], vec!["--single-thread"]] {
            let mut case = Case::new("session", "", &["pi", "session", "--offline", "--json"]);
            case.args.extend(flags.into_iter().map(str::to_string));
            let output = run_case(root, &seeded, &case);
            assert_eq!(output.exit, "0", "{}", output.stderr);
            let report: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
            let rows = report["sessions"].as_array().unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0]["sessionId"], "shared");
            assert_eq!(rows[0]["inputTokens"], 1000);
            assert_eq!(rows[0]["totalCost"], 0.003);
            assert!(rows[0]["modelBreakdowns"][0].get("storedCost").is_none());
        }
    }
}

#[test]
fn codex_daily_and_session_count_fork_requests_and_archived_copies_alike() {
    let fixture = Fixture::new();
    let root = fixture.root();
    let seeded = golden::codex::seed(root);
    let window = ["--since", "20251231", "--until", "20260101"];
    for (bounds, expected_total) in [(&[][..], 598_494), (&window[..], 19_855)] {
        for threads in [&[][..], &["--single-thread"][..]] {
            let report = |kind: &'static str| {
                let mut case = Case::new(kind, "", &["codex", kind, "--offline", "--json"]);
                case.args
                    .extend(bounds.iter().chain(threads).map(|arg| (*arg).to_string()));
                let output = run_case(root, &seeded, &case);
                assert_eq!(output.exit, "0", "{}", output.stderr);
                serde_json::from_str::<serde_json::Value>(&output.stdout).unwrap()
            };
            let context = format!("{bounds:?} {threads:?}");
            let daily = report("daily");
            let session = report("session");
            assert_eq!(daily["totals"]["totalTokens"], expected_total, "{context}");
            assert_eq!(
                session["totals"]["totalTokens"], expected_total,
                "{context}"
            );
            let cost = |report: &serde_json::Value| report["totals"]["costUSD"].as_f64().unwrap();
            let (daily_cost, session_cost) = (cost(&daily), cost(&session));
            assert!(
                (daily_cost - session_cost).abs() <= 1e-9 * daily_cost.abs().max(1.0),
                "{context}: daily {daily_cost} != session {session_cost}"
            );
            let sessions = session["sessions"].as_array().unwrap();
            let tokens = |id: &str| {
                sessions
                    .iter()
                    .find(|row| row["sessionId"].as_str().unwrap().ends_with(id))
                    .map(|row| row["totalTokens"].clone())
            };
            assert_eq!(
                tokens("33333333-3333-4333-8333-333333333333"),
                Some(1980.into()),
                "{context}"
            );
            assert_eq!(
                tokens("dddddddd-dddd-4ddd-8ddd-dddddddddddd"),
                None,
                "{context}"
            );
        }
    }
}

#[test]
fn pi_explicit_stores_keep_a_project_named_sessions_and_agent_dirs_read_their_store() {
    let fixture = Fixture::new();
    let root = fixture.root();
    let seeded = golden::pi::seed_store_with_sessions_project(root);
    let store = root.join(golden::pi::STORE_WITH_SESSIONS_PROJECT);
    let store = store.to_string_lossy();
    let agent = root.join(golden::pi::AGENT_DIR);
    let agent = agent.to_string_lossy();
    let config = |name: &str, pi: serde_json::Value| {
        let path = fixture.write_file(name, serde_json::json!({ "pi": pi }).to_string());
        path.to_string_lossy().into_owned()
    };
    let named = config(
        "named.json",
        serde_json::json!({"stores": [{"name": "omp", "path": store}]}),
    );
    let defaults = config(
        "defaults.json",
        serde_json::json!({"defaults": {"piAgentDir": agent}}),
    );
    let sessions = |env: &[(&'static str, Vec<std::ffi::OsString>)], args: &[&str]| {
        let mut case = Case::new("session", "", args);
        case.args
            .extend(["--offline", "--json"].map(str::to_string));
        let output = run_case(root, env, &case);
        assert_eq!(output.exit, "0", "{args:?}: {}", output.stderr);
        let report: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
        let rows = report["sessions"]
            .as_array()
            .or(report["session"].as_array());
        let mut sessions = rows
            .unwrap()
            .iter()
            .map(|row| {
                let id = row["sessionId"].as_str().or(row["period"].as_str());
                (
                    id.unwrap().to_string(),
                    row["inputTokens"].as_u64().unwrap(),
                )
            })
            .collect::<Vec<_>>();
        sessions.sort();
        sessions
    };
    let whole_store = vec![
        ("in-alpha".to_string(), 2000),
        ("in-sessions".to_string(), 1000),
    ];
    let agent_store = vec![("in-agent".to_string(), 4000)];

    assert_eq!(
        sessions(&seeded, &["pi", "session", "--pi-path", &store]),
        whole_store
    );
    assert_eq!(
        sessions(
            &[("PI_AGENT_DIR", vec![store.as_ref().into()])],
            &["pi", "session"]
        ),
        whole_store
    );
    assert_eq!(
        sessions(&seeded, &["session", "--config", &named]),
        whole_store
    );
    assert_eq!(
        sessions(&seeded, &["pi", "session", "--pi-agent-dir", &agent]),
        agent_store
    );
    assert_eq!(
        sessions(&seeded, &["pi", "session", "--config", &defaults]),
        agent_store
    );
}

#[test]
fn nested_claude_advisor_is_counted_once_with_a_top_level_copy() {
    let fixture = Fixture::new();
    let nested = serde_json::json!({
        "type": "assistant", "timestamp": "2026-01-02T10:00:00Z", "requestId": "req-advisor",
        "message": {"id": "msg-advisor", "model": "claude-sonnet-4-20250514", "usage": {
            "input_tokens": 100, "output_tokens": 1, "iterations": [{
                "type": "advisor_message", "model": "claude-opus-4-20250514", "input_tokens": 10, "output_tokens": 2
            }]
        }}
    });
    let wrapped = serde_json::json!({"type":"progress", "sessionId":"session-advisor", "data": {
        "type":"agent_progress", "message": nested
    }})
    .to_string();
    let transcript = fixture.write_file("claude/projects/p/session-advisor.jsonl", &wrapped);
    let seeded = vec![(
        "CLAUDE_CONFIG_DIR",
        vec![fixture.path("claude").into_os_string()],
    )];
    let daily = Case::new(
        "daily",
        "json_nested_advisor",
        &[
            "claude",
            "daily",
            "--offline",
            "--json",
            "--mode",
            "calculate",
        ],
    );
    let first = run_case(fixture.root(), &seeded, &daily);
    assert_eq!(first.exit, "0", "{}", first.stderr);
    let report: serde_json::Value = serde_json::from_str(&first.stdout).unwrap();
    assert_eq!(report["totals"]["inputTokens"], 110);
    assert_eq!(report["totals"]["outputTokens"], 3);
    golden::assert_case("claude", fixture.root(), &seeded, &daily);
    let cases: Vec<_> = ["daily", "session", "blocks"]
        .into_iter()
        .map(|kind| {
            Case::new(
                kind,
                "",
                &["claude", kind, "--offline", "--json", "--mode", "calculate"],
            )
        })
        .collect();
    let before: Vec<_> = cases
        .iter()
        .map(|case| run_case(fixture.root(), &seeded, case).stdout)
        .collect();
    let mut direct = nested;
    direct["sessionId"] = "session-advisor".into();
    std::fs::write(&transcript, format!("{wrapped}\n{direct}")).unwrap();
    for (case, expected) in cases.iter().zip(before) {
        let actual = run_case(fixture.root(), &seeded, case);
        assert_eq!(actual.exit, "0", "{}", actual.stderr);
        assert_eq!(actual.stdout, expected, "{}", case.kind);
    }
}

#[test]
fn all_agents_unified() {
    golden::all::run(AGENTS);
}

#[test]
#[ignore = "writes into CCUSAGE_GOLDEN_FIXTURE_DIR; run by diff-refs.bb"]
fn export_golden_fixture() {
    let dir = Environment::current()
        .var_os("CCUSAGE_GOLDEN_FIXTURE_DIR")
        .expect("CCUSAGE_GOLDEN_FIXTURE_DIR names the directory to export into");
    golden::export_fixture(AGENTS, std::path::Path::new(&dir));
}

#[test]
fn exported_fixture_replays_the_unified_report() {
    let fixture = Fixture::new();
    let root = fixture.root();
    golden::export_fixture(AGENTS, root);
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(golden::FIXTURE_MANIFEST)).unwrap())
            .unwrap();
    let env = manifest["env"].as_object().unwrap();
    let seeded = manifest["seeded"].as_array().unwrap();
    assert!(!seeded.is_empty());
    for name in seeded {
        let name = name.as_str().unwrap();
        let stores = env[name].as_str().unwrap();
        assert!(
            stores
                .split(',')
                .any(|store| std::path::Path::new(store.trim()).exists()),
            "{name}={stores} names no seeded store"
        );
    }

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ccusage"))
        .env_clear()
        .envs(
            env.iter()
                .map(|(name, value)| (name, value.as_str().unwrap())),
        )
        .current_dir(manifest["cwd"].as_str().unwrap())
        .args(["daily", "--offline", "--json", "--by-agent"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let reported: BTreeSet<&str> = report["daily"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|day| day["agents"].as_array().unwrap())
        .map(|agent| agent["agent"].as_str().unwrap())
        .collect();
    assert_eq!(
        reported,
        AGENTS.iter().map(|matrix| matrix.agent).collect(),
        "every agent's seed is visible through the exported environment"
    );
    assert_eq!(manifest["window"]["since"], golden::WINDOW_SINCE);
    assert_eq!(manifest["window"]["until"], golden::WINDOW_UNTIL);
}

#[test]
fn malformed_test_clock_is_a_usage_error_not_a_panic() {
    let fixture = Fixture::new();
    let output = golden::hermetic_command(fixture.root(), &[], golden::WIDE_COLUMNS)
        .env("CCUSAGE_TEST_NOW", "2026-01-02")
        .args(["daily", "--offline", "--json", "--last", "1"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.starts_with("CCUSAGE_TEST_NOW=2026-01-02 is not an RFC 3339 instant"),
        "{stderr}"
    );
    assert!(!stderr.contains("panicked"), "{stderr}");
}

#[test]
fn last_beyond_the_span_limits_is_a_usage_error_not_a_panic() {
    let fixture = Fixture::new();
    let output = golden::hermetic_command(fixture.root(), &[], golden::WIDE_COLUMNS)
        .args(["daily", "--offline", "--json", "--last", "4294967295"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.starts_with("Could not resolve --last 4294967295 from today's date"),
        "{stderr}"
    );
    assert!(!stderr.contains("panicked"), "{stderr}");
}

#[test]
fn claude_project_case_runs_a_successful_filtered_report() {
    let fixture = Fixture::new();
    let root = fixture.root();
    let seeded = golden::claude::seed(root);
    let case = golden::claude::extra_cases(root)
        .into_iter()
        .find(|case| case.variant == "json_project")
        .unwrap();
    let output = run_case(root, &seeded, &case);

    assert_eq!(output.exit, "0", "{}", output.stderr);
    let report: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
    let dates = report["daily"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["date"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(dates, ["2025-12-31", "2026-01-01", "2026-01-02"]);
}

#[test]
fn statusline_hook_paths_round_trip_as_json() {
    let root = std::path::Path::new(r#"C:\fixtures\a "quoted" directory"#);
    let cases = golden::claude::extra_cases(root);
    let payloads: Vec<_> = cases
        .iter()
        .filter_map(|case| case.stdin.as_ref())
        .collect();
    assert!(!payloads.is_empty());
    for payload in payloads {
        let hook: serde_json::Value = serde_json::from_str(payload).unwrap();
        assert_eq!(
            hook["transcript_path"].as_str().unwrap(),
            root.join("claude/projects/-Users-dev-alpha")
                .join("sess-alpha-1.jsonl")
                .to_str()
                .unwrap()
        );
    }
}

fn help_commands(help: &str) -> Vec<String> {
    help.lines()
        .skip_while(|line| line.trim() != "COMMANDS:")
        .skip(1)
        .take_while(|line| !line.trim().is_empty())
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

#[test]
fn every_accepted_agent_report_is_in_the_matrix() {
    let fixture = Fixture::new();
    let root = fixture.root();
    let run = |args: &[&str]| run_case(root, &[], &Case::new("", "", args));

    let commands = help_commands(&run(&["--help"]).stdout);
    assert!(
        !commands.is_empty(),
        "`ccusage --help` lists no commands; the help layout this guard parses has changed"
    );

    let (kinds, agents): (BTreeSet<&str>, BTreeSet<&str>) = commands
        .iter()
        .map(String::as_str)
        .partition(|command| ALL_REPORT_KINDS.contains(command));
    assert_eq!(
        kinds,
        BTreeSet::from(ALL_REPORT_KINDS),
        "top-level report commands in `ccusage --help` differ from golden::ALL_REPORT_KINDS"
    );
    assert_eq!(
        agents,
        AGENTS.iter().map(|matrix| matrix.agent).collect(),
        "agent commands in `ccusage --help` differ from the agents in agent_matrix!; \
         a new agent needs a seed in tests/golden and an entry there"
    );

    for matrix in AGENTS {
        let listed: BTreeSet<String> = help_commands(&run(&[matrix.agent, "--help"]).stdout)
            .into_iter()
            .collect();
        assert_eq!(
            listed,
            golden::report_kinds(matrix.agent)
                .iter()
                .map(|kind| (*kind).to_string())
                .collect(),
            "`ccusage {} --help` lists different reports than golden::report_kinds",
            matrix.agent
        );
    }

    let mut uncovered = Vec::new();
    for matrix in AGENTS {
        let snapshotted: BTreeSet<&str> = agent_cases(matrix.agent, matrix.extras, root)
            .iter()
            .filter(|case| {
                case.variant != UNSUPPORTED_VARIANT
                    && case.args.first().map(String::as_str) == Some(matrix.agent)
                    && case.args.get(1).map(String::as_str) == Some(case.kind)
            })
            .map(|case| case.kind)
            .collect();
        for kind in ALL_REPORT_KINDS {
            if snapshotted.contains(kind) {
                continue;
            }
            let probe = run(&[matrix.agent, kind, "--offline", "--json"]);
            if probe.exit != "2" {
                uncovered.push(format!("{} {kind}", matrix.agent));
            }
        }
    }
    assert!(
        uncovered.is_empty(),
        "the CLI accepts these reports but the golden matrix does not snapshot them \
         (see golden::report_kinds): {uncovered:?}"
    );
}
