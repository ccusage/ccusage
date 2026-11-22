//! Anonymous, opt-out usage telemetry.
//!
//! At most one ping per UTC day, carrying only the ccusage version, the OS and
//! CPU architecture, the command name, and a random ID. Nothing read from agent
//! logs is ever part of it. `CCUSAGE_TELEMETRY_DISABLED`, `DO_NOT_TRACK`, CI
//! environments, and `ccusage telemetry disable` all turn it off.

use std::{
    env, fs,
    fs::File,
    io,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use ccusage_telemetry::{Ping, PingEvent};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};

use crate::{
    Result,
    cli::{AgentReportKind, Command, TelemetryAction},
    http,
};

/// Builds without this endpoint never send anything, so forks and local builds
/// cannot report into the official collector by accident.
const ENDPOINT: Option<&str> = option_env!("CCUSAGE_TELEMETRY_ENDPOINT");
const DOCS_URL: &str = "https://ccusage.com/guide/telemetry";
const SEND_TIMEOUT: Duration = Duration::from_secs(3);
/// The longest a finished report waits for an unsent ping. The ping only happens
/// on the first run of a day, so this bounds a once-a-day delay.
const EXIT_WAIT: Duration = Duration::from_millis(500);
const SECONDS_PER_DAY: u64 = 86_400;
const DISABLE_VARS: [&str; 2] = ["CCUSAGE_TELEMETRY_DISABLED", "DO_NOT_TRACK"];

/// A ping running on a background thread.
pub(crate) struct PendingPing(mpsc::Receiver<()>);

impl PendingPing {
    /// Gives the ping a short window to finish before the process exits and
    /// kills the thread; a slow or failed ping is simply dropped.
    pub(crate) fn finish(self) {
        let _ = self.0.recv_timeout(EXIT_WAIT);
    }
}

/// Why telemetry is off. Checked in this order, so `telemetry status` names the
/// setting a user would actually have to change.
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
    /// `None` means the user never chose, which keeps telemetry on.
    #[serde(skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    anonymous_id: Option<String>,
    /// Unix seconds when the first-run notice was printed.
    #[serde(skip_serializing_if = "Option::is_none")]
    notified_at: Option<u64>,
    /// UTC day number (days since the Unix epoch) of the last ping.
    #[serde(skip_serializing_if = "Option::is_none")]
    last_sent_day: Option<u64>,
}

/// What one run should do, decided from the stored state.
#[derive(Debug, Eq, PartialEq)]
struct Plan {
    notice: bool,
    send: bool,
}

/// Starts today's ping if one is due, printing the first-run notice when needed.
///
/// Returns `None` whenever nothing was sent, including every disabled case.
pub(crate) fn start(command: Option<&Command>, offline: bool) -> Option<PendingPing> {
    if matches!(command, Some(Command::Telemetry(_))) {
        return None;
    }
    if offline || command_offline(command) {
        return None;
    }
    if env_disabled(env_var).is_some() {
        return None;
    }
    let url = ENDPOINT?;
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
    // Statusline output is read by Claude Code, not a person, so the notice
    // waits for the next interactive command instead of disappearing there.
    let can_notify = !matches!(command, Some(Command::Statusline(_)));
    let now = unix_seconds();
    // The first interactive invocation is notice-only, giving users a full
    // run to opt out before any ping leaves the machine.
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
    // A state that cannot be saved would mint a new ID and ping again on every
    // run, inflating the counts, so an unwritable state never sends.
    save_state(&path, &state).ok()?;
    drop(_lock);

    let body = serde_json::to_string(&payload(state.anonymous_id.as_deref()?, command)).ok()?;
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let _ = http::post_json(url, &body, SEND_TIMEOUT);
        let _ = done.send(());
    });
    Some(PendingPing(finished))
}

/// Runs `ccusage telemetry [status|enable|disable]`.
pub(crate) fn run(action: TelemetryAction) -> Result<()> {
    let path = state_path().ok_or_else(|| {
        crate::cli_error("Cannot locate a config directory for the telemetry setting")
    })?;
    let _lock = StateLock::acquire(&path)?;
    let mut state = load_state(&path)?;
    match action {
        TelemetryAction::Status => {}
        TelemetryAction::Enable => {
            state.enabled = Some(true);
            // Enabling it explicitly is consent enough; no notice afterwards.
            state.notified_at.get_or_insert_with(unix_seconds);
            save_state(&path, &state)?;
            println!("Telemetry enabled.");
        }
        TelemetryAction::Disable => {
            state.enabled = Some(false);
            // A later `enable` starts over as a new anonymous user.
            state.anonymous_id = None;
            state.last_sent_day = None;
            save_state(&path, &state)?;
            println!("Telemetry disabled.");
        }
    }
    print_status(&state, &path);
    Ok(())
}

fn print_status(state: &State, path: &Path) {
    match effective_disabled(state, env_var) {
        None => println!("Status: enabled"),
        Some(Disabled::EnvVar(name)) => println!("Status: disabled by {name}"),
        Some(Disabled::Ci) => println!("Status: disabled in CI"),
        Some(Disabled::Preference) => println!("Status: disabled by `ccusage telemetry disable`"),
        Some(Disabled::NotConfigured) => {
            println!("Status: off (this build has no telemetry endpoint and never sends data)")
        }
    }
    if let Some(id) = &state.anonymous_id {
        println!("Anonymous ID: {id}");
    }
    println!("Settings file: {}", path.display());
    println!("Learn more: {DOCS_URL}");
}

fn print_notice() {
    eprintln!(
        "ccusage collects anonymous usage data to count its users: at most one ping a day with \
         the ccusage version, OS, CPU architecture, command name, and a random ID. \
         Nothing from your usage logs is sent.\n\
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
        eprintln!(
            "[telemetry] would send to {}:",
            ENDPOINT.unwrap_or("<no endpoint>")
        );
        eprintln!("{body}");
    }
}

fn plan(state: &mut State, now: u64, can_notify: bool) -> Plan {
    let notice = can_notify && state.notified_at.is_none();
    if notice {
        state.notified_at = Some(now);
    }
    let today = now / SECONDS_PER_DAY;
    let send = !notice && state.notified_at.is_some() && state.last_sent_day != Some(today);
    if send {
        // Recorded before sending: a failed ping is not retried, and parallel
        // statusline runs do not all report the same day.
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

/// Names the command the way a user types it, so `ccusage daily` and
/// `ccusage claude daily` stay distinct even though both parse to report types.
fn command_name(command: Option<&Command>) -> String {
    let (agent, report) = match command {
        None => (None, "daily"),
        Some(Command::All(args)) => (None, report_name(args.kind)),
        Some(Command::Daily(_)) => (Some("claude"), "daily"),
        Some(Command::Monthly(_)) => (Some("claude"), "monthly"),
        Some(Command::Weekly(_)) => (Some("claude"), "weekly"),
        Some(Command::Session(_)) => (Some("claude"), "session"),
        Some(Command::Blocks(_)) => (Some("claude"), "blocks"),
        Some(Command::Statusline(_)) => (None, "statusline"),
        Some(Command::Telemetry(_)) => (None, "telemetry"),
        Some(Command::Codex(args)) => (Some("codex"), report_name(args.kind)),
        Some(Command::OpenCode(args)) => (Some("opencode"), report_name(args.kind)),
        Some(Command::Amp(args)) => (Some("amp"), report_name(args.kind)),
        Some(Command::Droid(args)) => (Some("droid"), report_name(args.kind)),
        Some(Command::Codebuff(args)) => (Some("codebuff"), report_name(args.kind)),
        Some(Command::Hermes(args)) => (Some("hermes"), report_name(args.kind)),
        Some(Command::Pi(args)) => (Some("pi"), report_name(args.kind)),
        Some(Command::Goose(args)) => (Some("goose"), report_name(args.kind)),
        Some(Command::Kilo(args)) => (Some("kilo"), report_name(args.kind)),
        Some(Command::Copilot(args)) => (Some("copilot"), report_name(args.kind)),
        Some(Command::Gemini(args)) => (Some("gemini"), report_name(args.kind)),
        Some(Command::Antigravity(args)) => (Some("antigravity"), report_name(args.kind)),
        Some(Command::Kimi(args)) => (Some("kimi"), report_name(args.kind)),
        Some(Command::Qwen(args)) => (Some("qwen"), report_name(args.kind)),
        Some(Command::OpenClaw(args)) => (Some("openclaw"), report_name(args.kind)),
        Some(Command::Grok(args)) => (Some("grok"), report_name(args.kind)),
        Some(Command::ZCode(args)) => (Some("zcode"), report_name(args.kind)),
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
    let shared = match command {
        Some(
            Command::All(args)
            | Command::Codex(args)
            | Command::OpenCode(args)
            | Command::Amp(args)
            | Command::Droid(args)
            | Command::Codebuff(args)
            | Command::Hermes(args)
            | Command::Pi(args)
            | Command::Goose(args)
            | Command::Kilo(args)
            | Command::Copilot(args)
            | Command::Gemini(args)
            | Command::Antigravity(args)
            | Command::Kimi(args)
            | Command::Qwen(args)
            | Command::OpenClaw(args)
            | Command::Grok(args)
            | Command::ZCode(args),
        ) => Some(&args.shared),
        Some(Command::Daily(args)) => Some(&args.shared),
        Some(Command::Monthly(args)) => Some(args),
        Some(Command::Weekly(args)) => Some(&args.shared),
        Some(Command::Session(args)) => Some(&args.shared),
        Some(Command::Blocks(args)) => Some(&args.shared),
        Some(Command::Statusline(args)) => return args.offline && !args.no_offline,
        None | Some(Command::Telemetry(_)) => None,
    };
    shared.is_some_and(|args| args.offline && !args.no_offline)
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

/// Treats any value except empty, `0`, and `false` as set, matching how
/// `DO_NOT_TRACK` and `CI` are used in practice.
fn truthy(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        let value = value.trim();
        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    })
}

fn env_var(name: &str) -> Option<String> {
    env::var(name).ok()
}

fn state_path() -> Option<PathBuf> {
    state_path_under(
        env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        ccusage_core::home::home_dir(),
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

/// A missing file is the never-configured default; an existing unreadable or
/// corrupt file fails closed so telemetry cannot silently re-enable itself.
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

/// Writes through a temporary file so a concurrent reader never sees half a file.
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
    fn statusline_does_not_send_before_a_notice() {
        let mut state = State::default();

        assert_eq!(
            plan(&mut state, 0, false),
            Plan {
                notice: false,
                send: false
            }
        );
        assert_eq!(state.notified_at, None);
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
        let agent_args = |kind| crate::cli::AgentCommandArgs {
            shared: crate::cli::SharedArgs::default(),
            kind,
            session_id: None,
            sections: None,
            by_agent: false,
            pi_path: None,
            open_claw_path: None,
            codex_speed: crate::cli::CodexSpeed::Auto,
        };

        assert_eq!(
            command_name(Some(&Command::All(agent_args(AgentReportKind::Monthly)))),
            "monthly"
        );
        assert_eq!(
            command_name(Some(&Command::Codex(agent_args(AgentReportKind::Session)))),
            "codex session"
        );
        assert_eq!(
            command_name(Some(&Command::Monthly(crate::cli::SharedArgs::default()))),
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
        assert_eq!(
            state_path_under(Some("/xdg".into()), Some("/home/me".into())),
            Some(PathBuf::from("/xdg/ccusage/telemetry.json"))
        );
        assert_eq!(
            state_path_under(Some("relative".into()), Some("/home/me".into())),
            Some(PathBuf::from("/home/me/.config/ccusage/telemetry.json"))
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
        let args = crate::cli::AgentCommandArgs {
            shared: crate::cli::SharedArgs {
                offline: true,
                ..crate::cli::SharedArgs::default()
            },
            kind: AgentReportKind::Daily,
            session_id: None,
            sections: None,
            by_agent: false,
            pi_path: None,
            open_claw_path: None,
            codex_speed: crate::cli::CodexSpeed::Auto,
        };

        assert!(command_offline(Some(&Command::Codex(args))));
    }

    #[test]
    fn state_lock_serializes_writers() {
        let fixture = fs_fixture!({});
        let path = fixture.path("telemetry.json");
        let first = StateLock::acquire(&path).unwrap();
        let (attempted, waiting) = std::sync::mpsc::channel();
        let second_path = path.clone();
        let second = std::thread::spawn(move || {
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
