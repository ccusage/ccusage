//! Golden matrix: freezes what the `ccusage` binary prints for every agent
//! across every report kind and a fixed set of flag variants, plus the unified
//! reports over all agents at once.
//!
//! `tests/golden_matrix.rs` is the only test binary that includes this module.

pub mod all;
pub mod amp;
pub mod antigravity;
pub mod claude;
pub mod cline;
pub mod codebuff;
pub mod codex;
pub mod copilot;
pub mod droid;
pub mod gemini;
pub mod goose;
pub mod grok;
pub mod hermes;
pub mod kilo;
pub mod kimi;
pub mod openclaw;
pub mod opencode;
pub mod pi;
pub mod qwen;
pub mod zcode;

use std::{
    collections::BTreeMap,
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, UNIX_EPOCH},
};

use ccusage_test_support::Fixture;

/// Writes one agent's fixture store under `root` and returns the environment
/// variables (name -> path) that point the CLI at it. Several adapters take a
/// comma-separated list, which a seed returns as one `PathBuf` holding the
/// joined text.
pub type Seed = fn(root: &Path) -> Vec<(&'static str, PathBuf)>;

/// Builds the cases only one agent has (its own options, or reports that need
/// stdin) from the fixture root.
pub type Extras = fn(root: &Path) -> Vec<Case>;

pub fn no_extras(_root: &Path) -> Vec<Case> {
    Vec::new()
}

pub fn json_path(path: &Path) -> String {
    serde_json::to_string(&path.to_string_lossy()).expect("fixture path is JSON serialisable")
}

/// Every report kind the parser knows, in snapshot order.
pub const ALL_REPORT_KINDS: [&str; 6] = [
    "daily",
    "weekly",
    "monthly",
    "session",
    "blocks",
    "statusline",
];

/// Mirror of `agent_report_supported` in
/// `rust/crates/ccusage-cli-parser/src/parser.rs`. The matrix also snapshots
/// the rejection of every kind left out here, so a drift between this table
/// and the parser shows up as a snapshot change.
pub fn report_kinds(agent: &str) -> &'static [&'static str] {
    match agent {
        "claude" => &ALL_REPORT_KINDS,
        "opencode" | "cline" => &["daily", "weekly", "monthly", "session"],
        "codex" | "amp" | "droid" | "codebuff" | "hermes" | "pi" | "goose" | "kilo" | "copilot"
        | "gemini" | "antigravity" | "kimi" | "qwen" | "openclaw" | "grok" | "zcode" => {
            &["daily", "monthly", "session"]
        }
        _ => panic!("unknown agent '{agent}'"),
    }
}

/// One agent's entry in the matrix.
pub struct AgentMatrix {
    pub agent: &'static str,
    pub seed: Seed,
    pub extras: Extras,
}

/// Directory-valued environment variables that relocate an agent's store. Each
/// one a seed does not return is pointed at an empty directory, so no adapter
/// can fall back to a default location under `HOME`.
///
/// `COPILOT_OTEL_FILE_EXPORTER_PATH` is absent on purpose: it names a file and
/// has no `HOME` fallback, so it stays unset unless the Copilot seed returns it.
pub const AGENT_HOME_ENV_VARS: [&str; 19] = [
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "OPENCODE_DATA_DIR",
    "AMP_DATA_DIR",
    "DROID_SESSIONS_DIR",
    "CODEBUFF_DATA_DIR",
    "HERMES_HOME",
    "PI_AGENT_DIR",
    "GOOSE_PATH_ROOT",
    "KILO_DATA_DIR",
    "COPILOT_HOME",
    "GEMINI_DATA_DIR",
    "ANTIGRAVITY_DATA_DIR",
    "KIMI_DATA_DIR",
    "QWEN_DATA_DIR",
    "OPENCLAW_DIR",
    "GROK_HOME",
    "ZCODE_HOME",
    "CLINE_DATA_DIR",
];

/// The shared fixture timeline. Every seed spreads its records over these UTC
/// days so one `--since/--until` window cuts through the middle of all of them.
///
/// - `2025-12-27`: optional stale day, in an earlier week under both the Sunday
///   and the Monday week start.
/// - `2025-12-30`: first day, before the window.
/// - `2025-12-31`: inside the window. A record at or after `15:00Z` is already
///   `2026-01-01` in Asia/Tokyo, so it crosses the day, month, and year there.
/// - `2026-01-01`: inside the window.
/// - `2026-01-02`: last day, after the window.
pub const WINDOW_SINCE: &str = "20251231";
pub const WINDOW_UNTIL: &str = "20260101";

/// "Now" for every case, on the last fixture day: Claude's `08:00Z` block is
/// still active, `blocks --recent` keeps only the last three days, and the
/// statusline has usage for today.
pub const FIXED_NOW: &str = "2026-01-02T12:00:00Z";

/// Variant name of the case that pins how an unsupported kind is rejected.
pub const UNSUPPORTED_VARIANT: &str = "unsupported";

pub const WIDE_COLUMNS: u16 = 200;
pub const NARROW_COLUMNS: u16 = 120;

/// One CLI invocation whose output is snapshotted as
/// `golden__<scope>__<kind>__<variant>`.
pub struct Case {
    pub kind: &'static str,
    pub variant: &'static str,
    /// Arguments after `ccusage`: the agent name first, or the report kind
    /// for a unified report.
    pub args: Vec<String>,
    pub columns: u16,
    pub stdin: Option<String>,
}

impl Case {
    pub fn new(kind: &'static str, variant: &'static str, args: &[&str]) -> Self {
        Self {
            kind,
            variant,
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
            columns: WIDE_COLUMNS,
            stdin: None,
        }
    }

    #[must_use]
    pub fn columns(mut self, columns: u16) -> Self {
        self.columns = columns;
        self
    }

    #[must_use]
    pub fn stdin(mut self, stdin: impl Into<String>) -> Self {
        self.stdin = Some(stdin.into());
        self
    }
}

/// The variants each agent runs for its supported `kind`, decided from the parser:
/// `daily`, `weekly`, `monthly`, `session`, and `blocks` take the shared
/// options (`--json`, `--breakdown`, `--since`, `--until`, `--mode`,
/// `--timezone`), while `statusline` takes none of them and
/// needs a hook payload on stdin, so it is covered by agent-specific cases.
pub fn matrix_cases(agent: &'static str, kind: &'static str) -> Vec<Case> {
    if kind == "statusline" {
        return Vec::new();
    }
    shared_variants(&[agent, kind], kind)
}

/// The shared variants for the command `command` (`[agent, kind]`, or `[kind]`
/// for a unified report).
pub fn shared_variants(command: &[&str], kind: &'static str) -> Vec<Case> {
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

/// Every case the matrix runs for `agent`: the shared variants of each
/// supported kind, the rejection of each unsupported one, and the agent's own
/// extras.
pub fn agent_cases(agent: &'static str, extras: Extras, root: &Path) -> Vec<Case> {
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

/// Runs the whole matrix for `agent` against the store `seed` writes.
pub fn run_matrix(agent: &'static str, seed: Seed, extras: Extras) {
    let fixture = Fixture::new();
    let root = fixture.root();
    let seeded = seed(root);
    for case in &agent_cases(agent, extras, root) {
        assert_case(agent, root, &seeded, case);
    }
}

/// Writes every agent's seed into one `root` and returns the variables that
/// point the CLI at all of them.
pub fn seed_all(agents: &[AgentMatrix], root: &Path) -> Vec<(&'static str, PathBuf)> {
    // Seeds write to disjoint directories but may name the same variable (the
    // OpenCode seed adds a Claude store of its own); every such variable is a
    // comma-separated list, so the values are joined.
    let mut joined: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    for matrix in agents {
        for (name, path) in (matrix.seed)(root) {
            joined
                .entry(name)
                .or_default()
                .push(path.to_string_lossy().into_owned());
        }
    }
    joined
        .into_iter()
        .map(|(name, paths)| (name, PathBuf::from(paths.join(","))))
        .collect()
}

/// Name of the file `export_fixture` writes beside the seeds.
pub const FIXTURE_MANIFEST: &str = "golden-env.json";

/// Seeds every agent into `root` and writes `FIXTURE_MANIFEST` there: the
/// working directory and environment `hermetic_command` runs the unified
/// reports under, the variables in it that point at seeds, and the matrix's
/// date window. `diff-refs.bb` replays
/// commands from it, so both read the same stores the goldens do.
pub fn export_fixture(agents: &[AgentMatrix], root: &Path) {
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

/// Builds a `ccusage` command that can see nothing but the fixture: a cleared
/// environment, every home and store variable inside `root`, and a working
/// directory inside `root` because config discovery reads `./.ccusage`.
pub fn hermetic_command(root: &Path, seeded: &[(&'static str, PathBuf)], columns: u16) -> Command {
    let dir = |name: &str| {
        let path = root.join(name);
        fs::create_dir_all(&path).expect("failed to create hermetic directory");
        path
    };

    let mut command = Command::new(env!("CARGO_BIN_EXE_ccusage"));
    command
        .env_clear()
        .current_dir(dir("cwd"))
        .env("HOME", dir("home"))
        .env("USERPROFILE", dir("home"))
        .env("XDG_CONFIG_HOME", dir("xdg-config"))
        .env("XDG_DATA_HOME", dir("xdg-data"))
        .env("XDG_CACHE_HOME", dir("xdg-cache"))
        .env("TMPDIR", dir("tmp"))
        .env("TZ", "UTC")
        .env("CCUSAGE_TEST_NOW", FIXED_NOW)
        // jiff reads named zones such as Asia/Tokyo from the zoneinfo database,
        // which the Nix test sandbox only provides through TZDIR.
        .envs(std::env::var_os("TZDIR").map(|zoneinfo| ("TZDIR", zoneinfo)))
        .env("COLUMNS", columns.to_string())
        .env("NO_COLOR", "1")
        .env("LOG_LEVEL", "0");

    for name in AGENT_HOME_ENV_VARS {
        if seeded.iter().any(|(seeded_name, _)| *seeded_name == name) {
            continue;
        }
        let empty = dir(&format!("empty/{name}"));
        if name == "CLAUDE_CONFIG_DIR" {
            // A Claude config directory without `projects/` is an error rather
            // than an empty store.
            fs::create_dir_all(empty.join("projects"))
                .expect("failed to create empty Claude projects directory");
        }
        command.env(name, empty);
    }
    for (name, path) in seeded {
        command.env(name, path);
    }
    command
}

/// Sets a fixture file's mtime, for adapters that prune or order by it.
pub fn set_modified(path: &Path, unix_seconds: u64) {
    let modified = UNIX_EPOCH + Duration::from_secs(unix_seconds);
    fs::File::options()
        .write(true)
        .open(path)
        .expect("failed to open fixture file")
        .set_modified(modified)
        .expect("failed to set fixture file mtime");
}

/// What one invocation printed, with the fixture root redacted.
pub struct Outcome {
    pub exit: String,
    pub stdout: String,
    pub stderr: String,
}

pub fn run_case(root: &Path, seeded: &[(&'static str, PathBuf)], case: &Case) -> Outcome {
    let mut child = hermetic_command(root, seeded, case.columns)
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

/// Snapshots one case as `golden__<scope>__<kind>__<variant>`, where `scope`
/// is an agent name or `all` for the unified reports.
pub fn assert_case(scope: &str, root: &Path, seeded: &[(&'static str, PathBuf)], case: &Case) {
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
    // Without these the snapshot name would carry the name of whichever test
    // binary included this file, and the files would land beside it.
    settings.set_prepend_module_to_snapshot(false);
    settings.set_snapshot_path("../snapshots");
    settings.set_omit_expression(true);
    settings.set_description(redact_root(
        root,
        &format!("COLUMNS={} ccusage {}", case.columns, case.args.join(" ")),
    ));
    settings.bind(|| {
        insta::assert_snapshot!(
            format!("golden__{scope}__{}__{}", case.kind, case.variant),
            rendered
        );
    });
}

/// Replaces the temporary fixture root, which differs on every run, in both
/// its given and its symlink-resolved spelling (`/var` vs `/private/var` on
/// macOS).
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
