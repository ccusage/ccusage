pub(crate) mod all;
pub(crate) mod amp;
pub(crate) mod antigravity;
pub(crate) mod claude;
pub(crate) mod cline;
pub(crate) mod codebuff;
pub(crate) mod codex;
pub(crate) mod copilot;
pub(crate) mod droid;
pub(crate) mod dsh;
pub(crate) mod gemini;
pub(crate) mod goose;
pub(crate) mod grok;
pub(crate) mod hermes;
pub(crate) mod kilo;
pub(crate) mod kimi;
pub(crate) mod openclaw;
pub(crate) mod opencode;
pub(crate) mod pi;
pub(crate) mod qwen;
pub(crate) mod zcode;

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    io::Write as _,
    path::Path,
    process::{Command, Stdio},
};

use ccusage_test_support::Fixture;

pub(crate) type Seed = fn(root: &Path) -> Vec<(&'static str, Vec<OsString>)>;

pub(crate) type Extras = fn(root: &Path) -> Vec<Case>;

pub(crate) fn no_extras(_root: &Path) -> Vec<Case> {
    Vec::new()
}

pub(crate) fn json_path(path: &Path) -> String {
    serde_json::to_string(&path.to_string_lossy()).expect("fixture path is JSON serialisable")
}

pub(crate) const ALL_REPORT_KINDS: [&str; 6] = [
    "daily",
    "weekly",
    "monthly",
    "session",
    "blocks",
    "statusline",
];

pub(crate) fn report_kinds(agent: &str) -> Vec<&'static str> {
    let spec = ccusage_cli::AGENTS
        .iter()
        .find(|spec| spec.name == agent)
        .expect("matrix agent is registered");
    ccusage_cli::AgentReportKind::ALL
        .into_iter()
        .map(|kind| kind.name())
        .chain(spec.extra_commands.iter().map(|command| command.name))
        .collect()
}

pub(crate) struct AgentMatrix {
    pub(crate) agent: &'static str,
    pub(crate) seed: Seed,
    pub(crate) extras: Extras,
}

// NOTE(2026-11-15): The shared UTC fixture window is bracketed by 2025-12-30 and 2026-01-02, with an optional stale day on 2025-12-27. Records on either side of 15:00Z on 2025-12-31 exercise the day, month, and year rollover in Asia/Tokyo.
pub(crate) const WINDOW_SINCE: &str = "20251231";
pub(crate) const WINDOW_UNTIL: &str = "20260101";

pub(crate) const FIXED_NOW: &str = "2026-01-02T12:00:00Z";

pub(crate) const UNSUPPORTED_VARIANT: &str = "unsupported";

pub(crate) const WIDE_COLUMNS: u16 = 200;
pub(crate) const NARROW_COLUMNS: u16 = 120;

pub(crate) struct Case {
    pub(crate) kind: &'static str,
    pub(crate) variant: &'static str,
    pub(crate) args: Vec<String>,
    pub(crate) columns: u16,
    pub(crate) stdin: Option<String>,
    pub(crate) env: Vec<(&'static str, &'static str)>,
}

impl Case {
    pub(crate) fn new(kind: &'static str, variant: &'static str, args: &[&str]) -> Self {
        Self {
            kind,
            variant,
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
            columns: WIDE_COLUMNS,
            stdin: None,
            env: Vec::new(),
        }
    }

    #[must_use]
    pub(crate) fn env(mut self, name: &'static str, value: &'static str) -> Self {
        self.env.push((name, value));
        self
    }

    #[must_use]
    pub(crate) fn columns(mut self, columns: u16) -> Self {
        self.columns = columns;
        self
    }

    #[must_use]
    pub(crate) fn stdin(mut self, stdin: impl Into<String>) -> Self {
        self.stdin = Some(stdin.into());
        self
    }
}

pub(crate) fn matrix_cases(agent: &'static str, kind: &'static str) -> Vec<Case> {
    if kind == "statusline" {
        return Vec::new();
    }
    shared_variants(&[agent, kind], kind)
}

pub(crate) fn shared_variants(command: &[&str], kind: &'static str) -> Vec<Case> {
    let case = |variant, extra: &[&str]| {
        let mut args = command.to_vec();
        args.push("--offline");
        args.extend_from_slice(extra);
        Case::new(kind, variant, &args)
    };
    vec![
        case("json", &["--json"]),
        case("table", &[]),
        case("table_breakdown", &["--breakdown"]).columns(NARROW_COLUMNS),
        case(
            "json_window",
            &["--json", "--since", WINDOW_SINCE, "--until", WINDOW_UNTIL],
        ),
        case("json_mode_calculate", &["--json", "--mode", "calculate"]),
        case("json_mode_display", &["--json", "--mode", "display"]),
        case("json_tz_tokyo", &["--json", "--timezone", "Asia/Tokyo"]),
    ]
}

fn windowed_table(kind: &'static str, agent: &'static str, options: &[&str]) -> Case {
    let mut args = [
        agent,
        kind,
        "--offline",
        "--since",
        WINDOW_SINCE,
        "--until",
        WINDOW_UNTIL,
    ]
    .to_vec();
    args.extend_from_slice(options);
    Case::new(kind, "table_window", &args).env("LOG_LEVEL", "3")
}

pub(crate) fn agent_cases(agent: &'static str, extras: Extras, root: &Path) -> Vec<Case> {
    let supported = report_kinds(agent);
    let mut cases = Vec::new();
    for kind in ALL_REPORT_KINDS {
        if supported.contains(&kind) {
            cases.extend(matrix_cases(agent, kind));
        } else {
            cases.push(Case::new(
                kind,
                UNSUPPORTED_VARIANT,
                &[agent, kind, "--offline", "--json"],
            ));
        }
    }
    cases.extend(extras(root));
    cases
}

pub(crate) fn run_matrix(agent: &'static str, seed: Seed, extras: Extras) {
    let fixture = Fixture::new();
    let root = fixture.root();
    let seeded = seed(root);
    for case in &agent_cases(agent, extras, root) {
        assert_case(agent, root, &seeded, case);
    }
}

pub(crate) fn seed_all(agents: &[AgentMatrix], root: &Path) -> Vec<(&'static str, Vec<OsString>)> {
    let mut lists: BTreeMap<&'static str, Vec<OsString>> = BTreeMap::new();
    for matrix in agents {
        for (name, paths) in (matrix.seed)(root) {
            lists.entry(name).or_default().extend(paths);
        }
    }
    lists.into_iter().collect()
}

pub(crate) const FIXTURE_MANIFEST: &str = "golden-env.json";

pub(crate) fn export_fixture(agents: &[AgentMatrix], root: &Path) {
    let seeded = seed_all(agents, root);
    let command = hermetic_command(root, &seeded, WIDE_COLUMNS);
    let env: serde_json::Map<String, serde_json::Value> = command
        .get_envs()
        .filter_map(|(name, value)| {
            Some((
                name.to_string_lossy().into_owned(),
                value?.to_string_lossy().into_owned().into(),
            ))
        })
        .collect();
    let cwd = command
        .get_current_dir()
        .expect("hermetic command has a working directory");
    let manifest = serde_json::json!({
        "cwd": cwd.to_string_lossy(),
        "env": env,
        "seeded": seeded.iter().map(|(name, _)| name).collect::<Vec<_>>(),
        "window": { "since": WINDOW_SINCE, "until": WINDOW_UNTIL },
    });
    fs::write(
        root.join(FIXTURE_MANIFEST),
        serde_json::to_string_pretty(&manifest).expect("manifest is JSON serialisable"),
    )
    .expect("failed to write golden fixture manifest");
}

pub(crate) fn hermetic_command(
    root: &Path,
    seeded: &[(&'static str, Vec<OsString>)],
    columns: u16,
) -> Command {
    let mut command = crate::support::hermetic_command(root);
    command
        .env("CCUSAGE_TEST_NOW", FIXED_NOW)
        .env("COLUMNS", columns.to_string())
        .env("LOG_LEVEL", "0")
        .envs(seeded.iter().map(|(name, paths)| {
            let mut joined = OsString::new();
            for (index, path) in paths.iter().enumerate() {
                if index > 0 {
                    joined.push(",");
                }
                joined.push(path);
            }
            (name, joined)
        }));
    command
}

pub(crate) struct Outcome {
    pub(crate) exit: String,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

pub(crate) fn run_case(
    root: &Path,
    seeded: &[(&'static str, Vec<OsString>)],
    case: &Case,
) -> Outcome {
    let mut child = hermetic_command(root, seeded, case.columns)
        .envs(case.env.iter().copied())
        .args(&case.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to run ccusage");
    let mut stdin = child.stdin.take().expect("child stdin is piped");
    stdin
        .write_all(case.stdin.as_deref().unwrap_or_default().as_bytes())
        .expect("failed to write ccusage stdin");
    drop(stdin);
    let output = child
        .wait_with_output()
        .expect("failed to wait for ccusage");

    Outcome {
        exit: output
            .status
            .code()
            .map_or_else(|| "signal".to_string(), |code| code.to_string()),
        stdout: redact_root(root, &String::from_utf8_lossy(&output.stdout)),
        stderr: redact_root(root, &String::from_utf8_lossy(&output.stderr)),
    }
}

pub(crate) fn assert_case(
    scope: &str,
    root: &Path,
    seeded: &[(&'static str, Vec<OsString>)],
    case: &Case,
) {
    assert_outcome(scope, root, case, run_case(root, seeded, case));
}

fn assert_outcome(scope: &str, root: &Path, case: &Case, outcome: Outcome) {
    let Outcome {
        exit,
        stdout,
        stderr,
    } = outcome;
    let rendered = format!("exit: {exit}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}");

    let mut settings = insta::Settings::clone_current();
    settings.set_prepend_module_to_snapshot(false);
    settings.set_snapshot_path("../snapshots");
    settings.set_omit_expression(true);
    let env: String = case
        .env
        .iter()
        .map(|(name, value)| format!("{name}={value} "))
        .collect();
    settings.set_description(redact_root(
        root,
        &format!(
            "{env}COLUMNS={} ccusage {}",
            case.columns,
            case.args.join(" ")
        ),
    ));
    settings.bind(|| {
        insta::assert_snapshot!(
            format!("golden__{scope}__{}__{}", case.kind, case.variant),
            rendered
        );
    });
}

fn redact_root(root: &Path, text: &str) -> String {
    let mut spellings = vec![root.to_string_lossy().into_owned()];
    if let Ok(canonical) = root.canonicalize() {
        spellings.push(canonical.to_string_lossy().into_owned());
    }
    let mut spellings: Vec<_> = spellings
        .into_iter()
        .flat_map(|spelling| {
            let quoted =
                serde_json::to_string(&spelling).expect("fixture root is JSON serialisable");
            let escaped = quoted[1..quoted.len() - 1].to_string();
            [spelling, escaped]
        })
        .collect();
    spellings.sort_by_key(|spelling| std::cmp::Reverse(spelling.len()));
    spellings.dedup();
    let redacted = spellings.iter().fold(text.to_string(), |text, spelling| {
        text.replace(spelling.as_str(), "[ROOT]")
    });
    let mut normalized = String::with_capacity(redacted.len());
    let mut tail = redacted.as_str();
    while let Some(index) = tail.find("[ROOT]") {
        let end = index + "[ROOT]".len();
        normalized.push_str(&tail[..end]);
        tail = &tail[end..];
        if tail.starts_with('\\') || tail.starts_with('/') {
            let end = tail
                .find(['\"', '\'', ':', '\n', '\r', '\t'])
                .unwrap_or(tail.len());
            normalized.push_str(&tail[..end].replace("\\\\", "/").replace('\\', "/"));
            tail = &tail[end..];
        }
    }
    normalized.push_str(tail);
    normalized
}

#[test]
fn redacts_windows_fixture_paths_without_rewriting_other_escapes() {
    let root = Path::new(r"C:\temp\fixture");
    assert_eq!(
        redact_root(
            root,
            r"Skipping 'C:\temp\fixture\antigravity\ide\empty.db': empty SQLite snapshot"
        ),
        "Skipping '[ROOT]/antigravity/ide/empty.db': empty SQLite snapshot"
    );
    assert_eq!(
        redact_root(
            root,
            r#"{"path":"C:\\temp\\fixture\\antigravity\\ide\\empty.db","model":"keep\\slashes","text":"line\nquote\""}"#
        ),
        r#"{"path":"[ROOT]/antigravity/ide/empty.db","model":"keep\\slashes","text":"line\nquote\""}"#
    );
}
