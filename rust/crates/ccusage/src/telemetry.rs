use std::{
    env, fs,
    fs::File,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use ccusage_cli::{
    AgentCommand, AgentReportKind, ClaudeReport, Command, TelemetryAction, environment::Environment,
};
use ccusage_telemetry::{Ping, PingEvent};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};

use crate::{Result, http};

const ENDPOINT: Option<&str> = option_env!("CCUSAGE_TELEMETRY_ENDPOINT");
const DOCS_URL: &str = "https://ccusage.com/guide/telemetry";
const SEND_TIMEOUT: Duration = Duration::from_secs(3);
const SENDER_VAR: &str = "CCUSAGE_TELEMETRY_SENDER";
const SECONDS_PER_DAY: u64 = 86_400;
const DISABLE_VARS: [&str; 2] = ["CCUSAGE_TELEMETRY_DISABLED", "DO_NOT_TRACK"];

#[derive(Debug, Eq, PartialEq)]
enum Disabled {
    EnvVar(&'static str),
    Ci,
    Preference,
    NotConfigured,
}

#[derive(Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", default)]
struct State {
    #[serde(skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    anonymous_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    notified_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_sent_day: Option<u64>,
}

#[derive(Debug, Eq, PartialEq)]
struct Plan {
    notice: bool,
    send: bool,
}

pub(crate) fn start(command: Option<&Command>) {
    let _ = send_daily_ping(command);
}

pub(crate) fn run_sender() -> bool {
    if env_var(SENDER_VAR).is_none() {
        return false;
    }
    let mut body = String::new();
    if let Some(url) = ENDPOINT
        && io::stdin().read_to_string(&mut body).is_ok()
    {
        let _ = http::post_json(url, &body, SEND_TIMEOUT);
    }
    true
}

fn send_daily_ping(command: Option<&Command>) -> Option<()> {
    if matches!(command, Some(Command::Telemetry(_))) {
        return None;
    }
    if command_offline(command) {
        return None;
    }
    if env_disabled(env_var).is_some() {
        return None;
    }
    ENDPOINT?;
    let path = state_path()?;
    let _lock = StateLock::try_acquire(&path).ok()?;
    let mut state = load_state(&path).ok()?;
    if state.enabled == Some(false) {
        return None;
    }
    if truthy(env_var("CCUSAGE_TELEMETRY_DEBUG").as_deref()) {
        print_debug_payload(&state, command);
        return None;
    }
    // NOTE(2026-11-25): Statusline output is consumed by Claude Code and redrawn every few seconds, so a notice there is never read. Statusline sends without one; the statusline and telemetry guides disclose it instead.
    let can_notify = !is_statusline(command);
    let now = unix_seconds();
    // NOTE(2026-11-22): The first eligible invocation only shows the notice. This lets users opt out before a ping.
    let plan = plan(&mut state, now, can_notify);
    if !plan.send {
        if plan.notice {
            save_state(&path, &state).ok()?;
            print_notice();
        }
        return None;
    }
    if state.anonymous_id.is_none() {
        state.anonymous_id = Some(new_anonymous_id()?);
    }
    save_state(&path, &state).ok()?;
    drop(_lock);

    let body = serde_json::to_string(&payload(state.anonymous_id.as_deref()?, command)).ok()?;
    spawn_sender(&body).ok()
}

// NOTE(2026-11-25): A sender thread dies when the command exits, and statusline exits within milliseconds. A detached process lets the request finish without delaying any command.
fn spawn_sender(body: &str) -> io::Result<()> {
    let mut command = std::process::Command::new(env::current_exe()?);
    command
        .env(SENDER_VAR, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach(&mut command);
    let mut child = command.spawn()?;
    let written = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("sender stdin is unavailable"))
        .and_then(|mut stdin| stdin.write_all(body.as_bytes()));
    ccusage_core::parallel::spawn_worker(move || child.wait());
    written
}

#[cfg(unix)]
fn detach(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(windows)]
fn detach(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
}

#[cfg(not(any(unix, windows)))]
fn detach(_command: &mut std::process::Command) {}

pub(crate) fn run(action: TelemetryAction) -> Result<()> {
    let path = state_path().ok_or_else(|| {
        ccusage_core::cli_error("Cannot locate a config directory for the telemetry setting")
    })?;
    let _lock = StateLock::acquire(&path)?;
    let mut state = load_state(&path)?;
    match action {
        TelemetryAction::Status => {}
        TelemetryAction::Enable => {
            state.enabled = Some(true);
            state.notified_at.get_or_insert_with(unix_seconds);
            save_state(&path, &state)?;
            ccusage_core::outln!("Telemetry enabled.")?;
        }
        TelemetryAction::Disable => {
            state.enabled = Some(false);
            state.anonymous_id = None;
            state.last_sent_day = None;
            save_state(&path, &state)?;
            ccusage_core::outln!("Telemetry disabled.")?;
        }
    }
    print_status(&state, &path)?;
    Ok(())
}

fn print_status(state: &State, path: &Path) -> Result<()> {
    match effective_disabled(state, env_var) {
        None => ccusage_core::outln!("Status: enabled")?,
        Some(Disabled::EnvVar(name)) => ccusage_core::outln!("Status: disabled by {name}")?,
        Some(Disabled::Ci) => ccusage_core::outln!("Status: disabled in CI")?,
        Some(Disabled::Preference) => {
            ccusage_core::outln!("Status: disabled by `ccusage telemetry disable`")?
        }
        Some(Disabled::NotConfigured) => ccusage_core::outln!(
            "Status: off (this build has no telemetry endpoint and never sends data)"
        )?,
    }
    if let Some(id) = &state.anonymous_id {
        ccusage_core::outln!("Anonymous ID: {id}")?;
    }
    ccusage_core::outln!("Settings file: {}", path.display())?;
    ccusage_core::outln!("Learn more: {DOCS_URL}")?;
    Ok(())
}

fn print_notice() {
    ccusage_core::errln!(
        "ccusage will send minimal usage statistics on a later run: at most one ping per UTC day with \
         version, OS, CPU architecture, first command name, and a random installation ID. \
         Nothing from your agent logs is sent.\n\
         Opt out with `ccusage telemetry disable` or CCUSAGE_TELEMETRY_DISABLED=1. \
         Learn more: {DOCS_URL}\n"
    );
}

fn print_debug_payload(state: &State, command: Option<&Command>) {
    let id = state
        .anonymous_id
        .as_deref()
        .unwrap_or("<generated on first send>");
    if let Ok(body) = serde_json::to_string_pretty(&payload(id, command)) {
        ccusage_core::errln!(
            "[telemetry] would send to {}:",
            ENDPOINT.unwrap_or("<no endpoint>")
        );
        ccusage_core::errln!("{body}");
    }
}

fn plan(state: &mut State, now: u64, can_notify: bool) -> Plan {
    let notice = can_notify && state.notified_at.is_none();
    if notice {
        state.notified_at = Some(now);
    }
    let today = now / SECONDS_PER_DAY;
    let send = !notice && state.last_sent_day != Some(today);
    if send {
        state.last_sent_day = Some(today);
    }
    Plan { notice, send }
}

fn payload(anonymous_id: &str, command: Option<&Command>) -> Ping {
    Ping {
        event: PingEvent::CliDailyActive,
        anonymous_id: anonymous_id.to_string(),
        version: env!("CCUSAGE_VERSION").to_string(),
        os: env::consts::OS.to_string(),
        arch: env::consts::ARCH.to_string(),
        command: command_name(command),
    }
}

fn command_name(command: Option<&Command>) -> String {
    let (agent, report) = match command {
        None => (None, "daily"),
        Some(Command::All(args)) => (None, report_name(args.kind)),
        Some(Command::Claude { report, .. }) => (
            Some("claude"),
            match report {
                ClaudeReport::Daily(_) => "daily",
                ClaudeReport::Monthly(_) => "monthly",
                ClaudeReport::Weekly(_) => "weekly",
                ClaudeReport::Session(_) => "session",
                ClaudeReport::Blocks(_) => "blocks",
            },
        ),
        Some(Command::Statusline(_)) => (None, "statusline"),
        Some(Command::Telemetry(_)) => (None, "telemetry"),
        Some(Command::Agent(agent, AgentCommand::Report(args))) => {
            (Some(agent.spec().name), report_name(args.kind))
        }
        Some(Command::Agent(agent, AgentCommand::Statusline(_))) => {
            (Some(agent.spec().name), "statusline")
        }
    };
    match agent {
        Some(agent) => format!("{agent} {report}"),
        None => report.to_string(),
    }
}

fn report_name(kind: AgentReportKind) -> &'static str {
    match kind {
        AgentReportKind::Daily => "daily",
        AgentReportKind::Weekly => "weekly",
        AgentReportKind::Monthly => "monthly",
        AgentReportKind::Session => "session",
    }
}

fn command_offline(command: Option<&Command>) -> bool {
    if let Some(Command::Statusline(args)) = command {
        return args.offline_selected;
    }
    command
        .and_then(Command::shared)
        .is_some_and(|args| args.offline_selected)
}

fn is_statusline(command: Option<&Command>) -> bool {
    matches!(
        command,
        Some(Command::Statusline(_) | Command::Agent(_, AgentCommand::Statusline(_)))
    )
}

fn effective_disabled(state: &State, lookup: impl Fn(&str) -> Option<String>) -> Option<Disabled> {
    env_disabled(lookup)
        .or_else(|| (state.enabled == Some(false)).then_some(Disabled::Preference))
        .or_else(|| ENDPOINT.is_none().then_some(Disabled::NotConfigured))
}

fn env_disabled(lookup: impl Fn(&str) -> Option<String>) -> Option<Disabled> {
    DISABLE_VARS
        .into_iter()
        .find(|name| truthy(lookup(name).as_deref()))
        .map(Disabled::EnvVar)
        .or_else(|| truthy(lookup("CI").as_deref()).then_some(Disabled::Ci))
}

fn truthy(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        let value = value.trim();
        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    })
}

fn env_var(name: &str) -> Option<String> {
    Environment::current().var(name).ok()
}

fn state_path() -> Option<PathBuf> {
    state_path_under(
        Environment::current()
            .var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from),
        ccusage_cli::home::home_dir(),
    )
}

fn state_path_under(xdg_config_home: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    xdg_config_home
        .filter(|path| path.is_absolute())
        .or_else(|| {
            home.filter(|path| path.is_absolute())
                .map(|home| home.join(".config"))
        })
        .map(|base| base.join("ccusage").join("telemetry.json"))
}

// NOTE(2026-11-22): Read errors are not the default state. Do not silently re-enable telemetry.
fn load_state(path: &Path) -> io::Result<State> {
    match fs::read_to_string(path) {
        Ok(contents) => serde_json::from_str(&contents)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(State::default()),
        Err(error) => Err(error),
    }
}

struct StateLock(File);

impl StateLock {
    fn acquire(path: &Path) -> io::Result<Self> {
        Self::open(path, File::lock)
    }

    fn try_acquire(path: &Path) -> io::Result<Self> {
        Self::open(path, |file| match file.try_lock() {
            Ok(()) => Ok(()),
            Err(std::fs::TryLockError::WouldBlock) => {
                Err(io::Error::from(io::ErrorKind::WouldBlock))
            }
            Err(std::fs::TryLockError::Error(error)) => Err(error),
        })
    }

    fn open(path: &Path, lock: fn(&File) -> io::Result<()>) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let lock_path = path.with_extension("lock");
        let file = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)?;
        lock(&file)?;
        Ok(Self(file))
    }
}

impl Drop for StateLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn save_state(path: &Path, state: &State) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    fs::write(&temporary, serde_json::to_string_pretty(state)?)?;
    fs::rename(&temporary, path).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })
}

fn new_anonymous_id() -> Option<String> {
    let mut bytes = [0u8; 16];
    SystemRandom::new().fill(&mut bytes).ok()?;
    Some(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use ccusage_test_support::fs_fixture;
    use serde_json::json;

    use super::*;

    fn lookup(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let vars = vars
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect::<HashMap<_, _>>();
        move |name| vars.get(name).cloned()
    }

    #[test]
    fn environment_opt_outs_win_in_documented_order() {
        assert_eq!(env_disabled(lookup(&[])), None);
        assert_eq!(
            env_disabled(lookup(&[("DO_NOT_TRACK", "1"), ("CI", "true")])),
            Some(Disabled::EnvVar("DO_NOT_TRACK"))
        );
        assert_eq!(
            env_disabled(lookup(&[
                ("CCUSAGE_TELEMETRY_DISABLED", "yes"),
                ("DO_NOT_TRACK", "1")
            ])),
            Some(Disabled::EnvVar("CCUSAGE_TELEMETRY_DISABLED"))
        );
        assert_eq!(env_disabled(lookup(&[("CI", "1")])), Some(Disabled::Ci));
    }

    #[test]
    fn empty_zero_and_false_do_not_count_as_set() {
        for value in ["", " ", "0", "false", "FALSE"] {
            assert_eq!(
                env_disabled(lookup(&[("DO_NOT_TRACK", value), ("CI", value)])),
                None,
                "{value:?}"
            );
        }
    }

    #[test]
    fn stored_preference_disables_after_the_environment() {
        let state = State {
            enabled: Some(false),
            ..State::default()
        };

        assert_eq!(
            effective_disabled(&state, lookup(&[])),
            Some(Disabled::Preference)
        );
        assert_eq!(
            effective_disabled(&state, lookup(&[("CI", "true")])),
            Some(Disabled::Ci)
        );
    }

    #[test]
    fn first_run_only_notifies_then_sends_on_the_following_run() {
        let mut state = State::default();
        let day = 20_000 * SECONDS_PER_DAY;

        assert_eq!(
            plan(&mut state, day + 10, true),
            Plan {
                notice: true,
                send: false
            }
        );
        assert_eq!(state.notified_at, Some(day + 10));
        assert_eq!(
            plan(&mut state, day + 3_600, true),
            Plan {
                notice: false,
                send: true
            }
        );
        assert_eq!(
            plan(&mut state, day + SECONDS_PER_DAY, true),
            Plan {
                notice: false,
                send: true
            }
        );
        assert_eq!(state.last_sent_day, Some(20_001));
    }

    #[test]
    fn statusline_sends_once_a_day_without_a_notice() {
        let mut state = State::default();
        let day = 20_000 * SECONDS_PER_DAY;

        assert_eq!(
            plan(&mut state, day, false),
            Plan {
                notice: false,
                send: true
            }
        );
        assert_eq!(
            plan(&mut state, day + 5, false),
            Plan {
                notice: false,
                send: false
            }
        );
        assert_eq!(state.notified_at, None);
        assert_eq!(
            plan(&mut state, day + 60, true),
            Plan {
                notice: true,
                send: false
            }
        );
    }

    #[test]
    fn payload_carries_only_the_documented_fields() {
        let body = serde_json::to_value(payload("abc", None)).unwrap();

        assert_eq!(
            body,
            json!({
                "event": "cli_daily_active",
                "anonymousId": "abc",
                "version": env!("CCUSAGE_VERSION"),
                "os": env::consts::OS,
                "arch": env::consts::ARCH,
                "command": "daily",
            })
        );
    }

    #[test]
    fn command_names_match_what_the_user_typed() {
        let agent_args = |kind| ccusage_cli::AgentCommandArgs {
            shared: ccusage_cli::SharedArgs::default(),
            kind,
            session_id: None,
            sections: None,
            by_agent: false,
            options: ccusage_cli::AgentOptions::default(),
            include_claude_agent: false,
            start_of_week: ccusage_cli::WeekDay::Monday,
            sources: Default::default(),
        };

        assert_eq!(
            command_name(Some(&Command::All(agent_args(AgentReportKind::Monthly)))),
            "monthly"
        );
        assert_eq!(
            command_name(Some(&Command::agent_report(
                ccusage_cli::StandardAgent::Codex,
                agent_args(AgentReportKind::Session)
            ))),
            "codex session"
        );
        assert_eq!(
            command_name(Some(&Command::claude_report(ClaudeReport::Monthly(
                ccusage_cli::SharedArgs::default()
            )))),
            "claude monthly"
        );
    }

    #[test]
    fn anonymous_ids_are_random_hex() {
        let first = new_anonymous_id().unwrap();
        let second = new_anonymous_id().unwrap();

        assert_eq!(first.len(), 32);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[test]
    fn state_path_prefers_absolute_xdg_then_home() {
        let fixture = fs_fixture!({});
        let xdg = fixture.path("xdg");
        let home = fixture.path("home");
        assert_eq!(
            state_path_under(Some(xdg.clone()), Some(home.clone())),
            Some(xdg.join("ccusage").join("telemetry.json"))
        );
        assert_eq!(
            state_path_under(Some("relative".into()), Some(home.clone())),
            Some(home.join(".config").join("ccusage").join("telemetry.json"))
        );
        assert_eq!(state_path_under(None, None), None);
    }

    #[test]
    fn state_round_trips_and_tolerates_a_corrupt_file() {
        let fixture = fs_fixture!({ "corrupt.json": "{not json" });
        let path = fixture.path("nested/telemetry.json");
        let state = State {
            enabled: Some(true),
            anonymous_id: Some("abc".to_string()),
            notified_at: Some(1),
            last_sent_day: Some(2),
        };

        save_state(&path, &state).unwrap();

        assert_eq!(load_state(&path).unwrap(), state);
        assert_eq!(
            load_state(&fixture.path("corrupt.json"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            load_state(&fixture.path("missing.json")).unwrap(),
            State::default()
        );
    }

    #[test]
    fn offline_commands_never_enter_the_send_plan() {
        let mut shared = ccusage_cli::SharedArgs::default();
        shared.select_offline(true);
        let args = ccusage_cli::AgentCommandArgs {
            shared,
            kind: AgentReportKind::Daily,
            session_id: None,
            sections: None,
            by_agent: false,
            options: ccusage_cli::AgentOptions::default(),
            include_claude_agent: false,
            start_of_week: ccusage_cli::WeekDay::Monday,
            sources: Default::default(),
        };

        assert!(command_offline(Some(&Command::agent_report(
            ccusage_cli::StandardAgent::Codex,
            args
        ))));
    }

    #[test]
    fn statusline_offline_blocks_only_when_the_user_selects_it() {
        let mut statusline = ccusage_cli::StatuslineArgs::default();
        assert!(statusline.offline);
        assert!(!command_offline(Some(&Command::Statusline(
            statusline.clone()
        ))));

        statusline.select_offline(true);
        assert!(command_offline(Some(&Command::Statusline(statusline))));
    }

    #[test]
    fn state_lock_serializes_writers() {
        let fixture = fs_fixture!({});
        let path = fixture.path("telemetry.json");
        let first = StateLock::acquire(&path).unwrap();
        let (attempted, waiting) = std::sync::mpsc::channel();
        let second_path = path.clone();
        let second = ccusage_core::parallel::spawn_worker(move || {
            attempted.send(()).unwrap();
            StateLock::try_acquire(&second_path)
        });
        waiting.recv().unwrap();
        let error = match second.join().unwrap() {
            Ok(_) => panic!("a second telemetry state lock was acquired"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        drop(first);
        assert!(StateLock::try_acquire(&path).is_ok());
    }
}
