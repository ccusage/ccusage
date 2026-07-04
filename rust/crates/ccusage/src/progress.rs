use std::{
    cell::RefCell,
    io::{self, IsTerminal, Write},
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPINNER_INTERVAL: Duration = Duration::from_millis(80);

thread_local! {
    static ACTIVE_PROGRESS: RefCell<Option<ProgressController>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UsageLoadAgent {
    Claude,
    Codex,
    OpenCode,
    Amp,
    Droid,
    Codebuff,
    Hermes,
    Pi,
    PiStore(&'static str),
    Goose,
    Kilo,
    Qwen,
    Copilot,
    Gemini,
    Kimi,
    OpenClaw,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LoadProgressState {
    Loading,
    Succeeded,
    Failed,
}

pub(crate) fn should_show_usage_load_progress(json: bool, output_is_tty: bool) -> bool {
    !json && output_is_tty
}

fn agent_label(agent: UsageLoadAgent) -> &'static str {
    match agent {
        UsageLoadAgent::Claude => "Claude",
        UsageLoadAgent::Codex => "Codex",
        UsageLoadAgent::OpenCode => "OpenCode",
        UsageLoadAgent::Amp => "Amp",
        UsageLoadAgent::Droid => "Droid",
        UsageLoadAgent::Codebuff => "Codebuff",
        UsageLoadAgent::Hermes => "Hermes",
        UsageLoadAgent::Pi => "pi-agent",
        UsageLoadAgent::PiStore(store) => store,
        UsageLoadAgent::Goose => "Goose",
        UsageLoadAgent::Kilo => "Kilo",
        UsageLoadAgent::Qwen => "Qwen",
        UsageLoadAgent::Copilot => "GitHub Copilot CLI",
        UsageLoadAgent::Gemini => "Gemini CLI",
        UsageLoadAgent::Kimi => "Kimi",
        UsageLoadAgent::OpenClaw => "OpenClaw",
    }
}

fn format_usage_load_progress_text(
    states: &[(UsageLoadAgent, LoadProgressState)],
    status: Option<&str>,
) -> String {
    if states.is_empty() {
        return status.unwrap_or("Loading usage logs").to_string();
    }

    let base = {
        let completed = states
            .iter()
            .filter(|(_, state)| !matches!(state, LoadProgressState::Loading))
            .count();
        let loading_agents = states
            .iter()
            .filter_map(|(agent, state)| {
                matches!(state, LoadProgressState::Loading).then_some(agent_label(*agent))
            })
            .collect::<Vec<_>>()
            .join(", ");
        if loading_agents.is_empty() {
            format!("Loading usage logs ({}/{})", completed, states.len())
        } else {
            format!(
                "Loading usage logs ({}/{}) :: {}",
                completed,
                states.len(),
                loading_agents
            )
        }
    };
    match status {
        Some(status) => format!("{status} :: {base}"),
        None => base,
    }
}

pub(crate) fn usage_load_output_is_tty() -> bool {
    io::stdout().is_terminal()
}

pub(crate) struct UsageLoadProgress {
    enabled: bool,
    controller: Option<ProgressController>,
    owns_session: bool,
    running: Option<Arc<(Mutex<bool>, Condvar)>>,
    worker: Option<JoinHandle<()>>,
    stopped: bool,
}

#[derive(Clone)]
struct ProgressController {
    state: Arc<Mutex<ProgressState>>,
}

#[derive(Default)]
struct ProgressState {
    status: Option<String>,
    states: Vec<(UsageLoadAgent, LoadProgressState)>,
    frame: usize,
    rendered: bool,
}

impl UsageLoadProgress {
    pub(crate) fn new(enabled: bool) -> Self {
        if !enabled {
            return Self {
                enabled,
                controller: None,
                owns_session: false,
                running: None,
                worker: None,
                stopped: false,
            };
        }
        if let Some(controller) = ACTIVE_PROGRESS.with(|active| active.borrow().clone()) {
            return Self {
                enabled,
                controller: Some(controller),
                owns_session: false,
                running: None,
                worker: None,
                stopped: false,
            };
        }

        let state = Arc::new(Mutex::new(ProgressState::default()));
        let controller = ProgressController {
            state: Arc::clone(&state),
        };
        let running = Arc::new((Mutex::new(true), Condvar::new()));
        let worker = {
            let running = Arc::clone(&running);
            Some(thread::spawn(move || {
                loop {
                    if let Ok(mut state) = state.lock()
                        && state.has_content()
                    {
                        state.render();
                    }
                    let (lock, cvar) = &*running;
                    let Ok(guard) = lock.lock() else {
                        break;
                    };
                    if !*guard {
                        break;
                    }
                    let Ok((guard, _)) = cvar.wait_timeout(guard, SPINNER_INTERVAL) else {
                        break;
                    };
                    if !*guard {
                        break;
                    }
                }
            }))
        };
        ACTIVE_PROGRESS.with(|active| {
            *active.borrow_mut() = Some(controller.clone());
        });
        Self {
            enabled,
            controller: Some(controller),
            owns_session: true,
            running: Some(running),
            worker,
            stopped: false,
        }
    }

    pub(crate) fn start(&mut self, agent: UsageLoadAgent) {
        self.set_state(agent, LoadProgressState::Loading);
    }

    pub(crate) fn succeed(&mut self, agent: UsageLoadAgent) {
        self.set_state(agent, LoadProgressState::Succeeded);
    }

    pub(crate) fn fail(&mut self, agent: UsageLoadAgent) {
        self.set_state(agent, LoadProgressState::Failed);
    }

    pub(crate) fn stop(&mut self) {
        if !self.enabled || self.stopped {
            return;
        }
        self.stopped = true;
        if !self.owns_session {
            return;
        }
        if let Some(running) = self.running.take() {
            let (lock, cvar) = &*running;
            if let Ok(mut is_running) = lock.lock() {
                *is_running = false;
                cvar.notify_all();
            }
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Some(controller) = self.controller.as_ref()
            && let Ok(mut state) = controller.state.lock()
        {
            if state.rendered {
                let _ = write!(io::stderr(), "\r\x1b[K\x1b[?25h");
                let _ = io::stderr().flush();
            }
            state.status = None;
            state.states.clear();
        }
        ACTIVE_PROGRESS.with(|active| {
            *active.borrow_mut() = None;
        });
    }

    pub(crate) fn set_status(&mut self, status: Option<String>) {
        let Some(controller) = self.controller.as_ref() else {
            return;
        };
        let Ok(mut state) = controller.state.lock() else {
            return;
        };
        state.status = status;
    }

    fn set_state(&mut self, agent: UsageLoadAgent, state: LoadProgressState) {
        let Some(controller) = self.controller.as_ref() else {
            return;
        };
        let Ok(mut progress_state) = controller.state.lock() else {
            return;
        };
        if let Some((_, current)) = progress_state
            .states
            .iter_mut()
            .find(|(current_agent, _)| *current_agent == agent)
        {
            *current = state;
        } else {
            progress_state.states.push((agent, state));
        }
    }
}

impl Drop for UsageLoadProgress {
    fn drop(&mut self) {
        self.stop();
    }
}

impl ProgressState {
    fn has_content(&self) -> bool {
        self.status.is_some() || !self.states.is_empty()
    }

    fn render(&mut self) {
        if !self.has_content() {
            return;
        }
        let text = format_usage_load_progress_text(&self.states, self.status.as_deref());
        let frame = SPINNER_FRAMES[self.frame % SPINNER_FRAMES.len()];
        self.frame = self.frame.wrapping_add(1);
        let _ = write!(
            io::stderr(),
            "\r\x1b[K\x1b[?25l\x1b[36m{frame}\x1b[39m {text}"
        );
        let _ = io::stderr().flush();
        self.rendered = true;
    }
}

pub(crate) fn track_usage_load<T, E>(
    agent: UsageLoadAgent,
    json: bool,
    load: impl FnOnce() -> std::result::Result<T, E>,
) -> std::result::Result<T, E> {
    let enabled = crate::log_level() != Some(0)
        && should_show_usage_load_progress(json, usage_load_output_is_tty());
    let mut progress = UsageLoadProgress::new(enabled);
    progress.start(agent);
    let result = load();
    match &result {
        Ok(_) => progress.succeed(agent),
        Err(_) => progress.fail(agent),
    }
    progress.stop();
    result
}

pub(crate) fn track_status<T>(
    enabled: bool,
    status: impl Into<String>,
    run: impl FnOnce() -> T,
) -> T {
    let mut progress = UsageLoadProgress::new(enabled);
    progress.set_status(Some(status.into()));
    let result = run();
    progress.set_status(None);
    progress.stop();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_active_agent_progress_with_completed_count() {
        let states = [
            (UsageLoadAgent::Claude, LoadProgressState::Succeeded),
            (UsageLoadAgent::Codex, LoadProgressState::Loading),
            (UsageLoadAgent::OpenCode, LoadProgressState::Loading),
        ];

        assert_eq!(
            format_usage_load_progress_text(&states, None),
            "Loading usage logs (1/3) :: Codex, OpenCode"
        );
    }

    #[test]
    fn includes_pricing_status_in_progress_text() {
        let states = [
            (UsageLoadAgent::Claude, LoadProgressState::Loading),
            (UsageLoadAgent::Codex, LoadProgressState::Loading),
        ];

        assert_eq!(
            format_usage_load_progress_text(
                &states,
                Some("Refreshing model pricing from LiteLLM...")
            ),
            "Refreshing model pricing from LiteLLM... :: Loading usage logs (0/2) :: Claude, Codex"
        );
    }

    #[test]
    fn renders_standalone_status_without_usage_suffix() {
        assert_eq!(
            format_usage_load_progress_text(&[], Some("Refreshing model pricing from LiteLLM...")),
            "Refreshing model pricing from LiteLLM..."
        );
    }

    #[test]
    fn hides_progress_for_json_or_non_tty_output() {
        assert!(!should_show_usage_load_progress(true, true));
        assert!(!should_show_usage_load_progress(false, false));
        assert!(should_show_usage_load_progress(false, true));
    }
}
