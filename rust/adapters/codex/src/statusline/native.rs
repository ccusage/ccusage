use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::PathBuf,
    sync::{Arc, Condvar, Mutex, PoisonError},
    time::SystemTime,
};

use ccusage_adapter_common::{source::ReportPricing, units::UnitError};
use ccusage_core::{
    Diagnostics, Result,
    cli::{AgentReportKind, CodexStatuslineArgs, DateBound, SharedArgs},
    date_at, format_currency, format_number, log_level, resolve_tz, sanitize_terminal_text,
};
use serde_json::Value;

use crate::{
    aggregate::{load_groups_from_file_selection, select_session_files},
    paths, report, resolve_codex_speed,
    speed::CodexSpeedPolicy,
};

#[derive(Default)]
struct State {
    pending: VecDeque<String>,
    stopped: bool,
    output: HashMap<String, String>,
}

pub(super) struct NativeUsageReader {
    state: Arc<(Mutex<State>, Condvar)>,
}

impl NativeUsageReader {
    pub(super) fn new(args: CodexStatuslineArgs) -> Self {
        let state = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let worker = Arc::clone(&state);
        ccusage_core::parallel::spawn_worker(move || {
            let pricing = ReportPricing::for_load(&args.shared, None);
            let mut cached = None;
            let mut last_warnings = Vec::new();
            loop {
                let thread_id = {
                    let (lock, ready) = &*worker;
                    let state = lock.lock().unwrap_or_else(PoisonError::into_inner);
                    let mut state = ready
                        .wait_while(state, |state| state.pending.is_empty() && !state.stopped)
                        .unwrap_or_else(PoisonError::into_inner);
                    if state.stopped {
                        return;
                    }
                    let Some(thread_id) = state.pending.pop_front() else {
                        continue;
                    };
                    thread_id
                };
                let now = args.shared.clock.now();
                let diagnostics = Diagnostics::new("codex");
                let timezone = resolve_tz(args.shared.timezone.as_ref());
                let today = date_at(now.into(), &timezone).into();
                let result = refresh(
                    &args,
                    &pricing,
                    &thread_id,
                    &mut cached,
                    &diagnostics,
                    today,
                );
                let (output, warnings) =
                    refresh_output(result, &diagnostics, &args.shared, &mut last_warnings);
                for warning in warnings {
                    ccusage_core::errln!("{warning}");
                }
                let mut state = worker.0.lock().unwrap_or_else(PoisonError::into_inner);
                state.output.insert(thread_id, output);
            }
        });
        Self { state }
    }

    pub(super) fn read(&self, thread_id: &str) -> String {
        let mut state = self.state.0.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.pending.iter().any(|id| id == thread_id) {
            state.pending.push_back(thread_id.to_string());
        }
        self.state.1.notify_one();
        state
            .output
            .get(thread_id)
            .cloned()
            .unwrap_or_else(|| "Codex | Loading usage...".to_string())
    }
}

impl Drop for NativeUsageReader {
    fn drop(&mut self) {
        self.state
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .stopped = true;
        self.state.1.notify_one();
    }
}

fn refresh_output<'a>(
    result: Result<String>,
    diagnostics: &Diagnostics,
    shared: &SharedArgs,
    last_warnings: &'a mut Vec<String>,
) -> (String, &'a [String]) {
    let mut warnings = diagnostics.warnings(shared);
    let output = match result {
        Ok(output) => output,
        Err(error) => {
            if log_level() != Some(0) || shared.debug {
                let message = sanitize_terminal_text(&error.to_string());
                warnings.push(format!("WARN  codex: statusline refresh failed: {message}"));
            }
            "Codex | Usage unavailable".to_string()
        }
    };
    if warnings == *last_warnings {
        (output, &[])
    } else {
        *last_warnings = warnings;
        (output, last_warnings)
    }
}

#[derive(PartialEq, Eq)]
struct Fingerprint {
    thread_id: String,
    today: ccusage_core::cli::DateBound,
    speed: CodexSpeedPolicy,
    files: Vec<(PathBuf, u64, SystemTime)>,
}

struct CachedUsage {
    fingerprint: Fingerprint,
    text: String,
    load_diagnostics: Diagnostics,
}

fn refresh(
    args: &CodexStatuslineArgs,
    pricing: &ReportPricing<'_>,
    thread_id: &str,
    cached: &mut Option<CachedUsage>,
    diagnostics: &Diagnostics,
    today: DateBound,
) -> Result<String> {
    let mut shared = args.shared.clone();
    shared.since = None;
    shared.until = None;
    let file_groups =
        paths::collect_deduped_codex_usage_files(&paths::codex_usage_sources()?, diagnostics);
    let speed = resolve_codex_speed(args.codex_speed);
    let files = file_groups
        .iter()
        .flat_map(|group| &group.files)
        .filter_map(|path| {
            match fs::metadata(path).and_then(|metadata| Ok((metadata.len(), metadata.modified()?)))
            {
                Ok((len, modified)) => Some((path.clone(), len, modified)),
                Err(error) => {
                    UnitError::from(error).record(path, diagnostics);
                    None
                }
            }
        })
        .collect();
    let fingerprint = Fingerprint {
        thread_id: thread_id.to_string(),
        today,
        speed,
        files,
    };
    if let Some(cached) = cached
        .as_ref()
        .filter(|cached| cached.fingerprint == fingerprint)
    {
        diagnostics.merge(&cached.load_diagnostics);
        return Ok(cached.text.clone());
    }

    let load_diagnostics = diagnostics.isolated();
    let session_files = select_session_files(&file_groups, thread_id, true);
    let pricing = pricing.get();
    let session = load_groups_from_file_selection(
        &file_groups,
        &session_files,
        &shared,
        AgentReportKind::Session,
        pricing,
        &load_diagnostics,
    )?;
    shared.since = Some(today);
    shared.until = Some(today);
    let today_files = file_groups
        .iter()
        .map(|group| paths::filter_codex_usage_files(group.source.dir(), &group.files, &shared))
        .collect::<Vec<_>>();
    let today = load_groups_from_file_selection(
        &file_groups,
        &today_files,
        &shared,
        AgentReportKind::Daily,
        pricing,
        &load_diagnostics,
    )?;
    diagnostics.merge(&load_diagnostics);
    let session = report::report_from_groups(
        &session,
        AgentReportKind::Session,
        pricing,
        speed,
        shared.mode,
    )["totals"]
        .take();
    let today = report::report_from_groups(
        &today,
        AgentReportKind::Daily,
        pricing,
        speed,
        shared.mode,
    )["totals"]
        .take();
    let text = format_native(&session, &today, shared.no_cost);
    *cached = Some(CachedUsage {
        fingerprint,
        text: text.clone(),
        load_diagnostics,
    });
    Ok(text)
}

fn format_native(session: &Value, today: &Value, no_cost: bool) -> String {
    let tokens = format_number(session["totalTokens"].as_u64().unwrap_or_default());
    if no_cost {
        return format!(
            "Codex | Session: {tokens} tokens | Today: {} tokens",
            format_number(today["totalTokens"].as_u64().unwrap_or_default())
        );
    }
    let mut text = format!(
        "Codex | Est. {} session / {} today | {tokens} tokens",
        format_currency(session["costUSD"].as_f64().unwrap_or_default()),
        format_currency(today["costUSD"].as_f64().unwrap_or_default()),
    );
    if [session, today].iter().any(|summary| {
        summary["unpricedModels"]
            .as_array()
            .is_some_and(|models| !models.is_empty())
    }) {
        text.push_str(" (incomplete pricing)");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_failure_is_reported_once_until_the_warning_changes() {
        let diagnostics = Diagnostics::new("codex");
        let shared = SharedArgs {
            debug: true,
            ..Default::default()
        };
        let mut last = Vec::new();
        for expected in [1, 0] {
            let (output, warnings) = refresh_output(
                Err(ccusage_core::cli_error("fake refresh failure")),
                &diagnostics,
                &shared,
                &mut last,
            );
            assert_eq!(output, "Codex | Usage unavailable");
            assert_eq!(warnings.len(), expected);
            if let Some(warning) = warnings.first() {
                assert!(warning.ends_with("fake refresh failure"), "{warning}");
            }
        }
        let (_, warnings) =
            refresh_output(Ok("usage".to_string()), &diagnostics, &shared, &mut last);
        assert!(warnings.is_empty());
        let (_, warnings) = refresh_output(
            Err(ccusage_core::cli_error("fake refresh failure")),
            &diagnostics,
            &shared,
            &mut last,
        );
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn a_poisoned_state_still_serves_cached_usage_and_shuts_down() {
        let state = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let reader = NativeUsageReader {
            state: Arc::clone(&state),
        };
        let poisoned = Arc::clone(&state);
        let result = ccusage_core::parallel::spawn_worker(move || {
            let mut state = poisoned.0.lock().unwrap();
            state
                .output
                .insert("thread".to_string(), "usage".to_string());
            panic!("poison the state");
        })
        .join();
        assert!(result.is_err());
        assert_eq!(reader.read("thread"), "usage");
        drop(reader);
        assert!(
            state
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .stopped
        );
    }
}
