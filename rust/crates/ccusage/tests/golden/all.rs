use ccusage_test_support::Fixture;

use crate::golden::{AgentMatrix, Case, NARROW_COLUMNS, assert_case, seed_all, shared_variants};

const UNIFIED_KINDS: [&str; 4] = ["daily", "weekly", "monthly", "session"];

pub(crate) fn run(agents: &[AgentMatrix]) {
    let fixture = Fixture::new();
    let root = fixture.root();
    let seeded = seed_all(agents, root);
    for case in &cases() {
        assert_case("all", root, &seeded, case);
    }
}

fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for kind in UNIFIED_KINDS {
        cases.extend(shared_variants(&[kind], kind));
        cases.extend([
            Case::new(
                kind,
                "json_by_agent",
                &[kind, "--offline", "--json", "--by-agent"],
            ),
            Case::new(
                kind,
                "json_no_cost",
                &[kind, "--offline", "--json", "--no-cost"],
            ),
            Case::new(kind, "table_compact", &[kind, "--offline", "--compact"])
                .columns(NARROW_COLUMNS),
        ]);
    }
    cases.extend([
        Case::new("daily", "json_implicit_command", &["--offline", "--json"]),
        Case::new(
            "daily",
            "json_all_flag",
            &["daily", "--all", "--offline", "--json"],
        ),
        Case::new(
            "daily",
            "json_sections",
            &[
                "daily",
                "--offline",
                "--json",
                "--sections",
                "daily,weekly,monthly,session",
            ],
        ),
        Case::new(
            "daily",
            "table_sections",
            &["--offline", "--sections", "monthly,session"],
        ),
        Case::new(
            "weekly",
            "json_start_sunday",
            &["weekly", "--offline", "--json", "--start-of-week", "sunday"],
        ),
        Case::new(
            "session",
            "id_json",
            &["session", "--offline", "--json", "--id", "sess-alpha-1"],
        ),
        Case::new(
            "session",
            "id_with_by_agent",
            &[
                "session",
                "--offline",
                "--json",
                "--by-agent",
                "--id",
                "sess-alpha-1",
            ],
        ),
        Case::new("blocks", "json", &["blocks", "--offline", "--json"]),
        Case::new(
            "statusline",
            "no_input",
            &["statusline", "--offline", "--no-cache"],
        ),
    ]);
    cases
}
