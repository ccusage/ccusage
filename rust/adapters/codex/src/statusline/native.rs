use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    thread,
    time::SystemTime,
};

use ccusage_core::{
    PricingMap, Result,
    cli::{AgentReportKind, CodexStatuslineArgs},
    format_currency, format_date_tz, format_number, parse_tz, utc_now,
};
use jiff::tz::TimeZone;
use serde_json::Value;

use crate::{
    aggregate::load_groups_from_file_selection, parser, paths, report, resolve_codex_speed,
    rollout_identity, speed::CodexSpeedPolicy,
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
        thread::spawn(move || {
            let pricing = super::load_pricing(&args);
            let mut cached = None;
            loop {
                let thread_id = {
                    let (lock, ready) = &*worker;
                    let mut state = ready
                        .wait_while(lock.lock().unwrap(), |state| {
                            state.pending.is_empty() && !state.stopped
                        })
                        .unwrap();
                    if state.stopped {
                        return;
                    }
                    state.pending.pop_front().unwrap()
                };
                let output = refresh(&args, &pricing, &thread_id, &mut cached)
                    .unwrap_or_else(|_| "Codex | Usage unavailable".to_string());
                let mut state = worker.0.lock().unwrap();
                state.output.insert(thread_id, output);
            }
        });
        Self { state }
    }

    pub(super) fn read(&self, thread_id: &str) -> String {
        let mut state = self.state.0.lock().unwrap();
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
        self.state.0.lock().unwrap().stopped = true;
        self.state.1.notify_one();
    }
}

#[derive(PartialEq, Eq)]
struct Fingerprint {
    thread_id: String,
    today: String,
    speed: CodexSpeedPolicy,
    files: Vec<(PathBuf, u64, SystemTime)>,
}

struct CachedUsage {
    fingerprint: Fingerprint,
    text: String,
}

fn refresh(
    args: &CodexStatuslineArgs,
    pricing: &PricingMap,
    thread_id: &str,
    cached: &mut Option<CachedUsage>,
) -> Result<String> {
    let mut shared = args.shared.clone();
    shared.since = None;
    shared.until = None;
    let timezone = parse_tz(shared.timezone.as_deref()).unwrap_or_else(TimeZone::system);
    let today = format_date_tz(utc_now(), Some(&timezone)).replace('-', "");
    let file_groups = paths::collect_deduped_codex_usage_files(&paths::codex_usage_sources()?);
    let speed = resolve_codex_speed(args.codex_speed);
    let files = file_groups
        .iter()
        .flat_map(|group| &group.files)
        .map(|path| {
            let metadata = fs::metadata(path)?;
            Ok((path.clone(), metadata.len(), metadata.modified()?))
        })
        .collect::<Result<Vec<_>>>()?;
    let fingerprint = Fingerprint {
        thread_id: thread_id.to_string(),
        today: today.clone(),
        speed,
        files,
    };
    if let Some(cached) = cached
        .as_ref()
        .filter(|cached| cached.fingerprint == fingerprint)
    {
        return Ok(cached.text.clone());
    }

    let session_files = file_groups
        .iter()
        .map(|group| {
            group
                .files
                .iter()
                .filter(|path| {
                    rollout_identity(&parser::codex_session_id(&group.dir, path))
                        .is_some_and(|(id, _)| id == thread_id)
                })
                .cloned()
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let session = load_groups_from_file_selection(
        &file_groups,
        &session_files,
        &shared,
        AgentReportKind::Session,
    )?;
    shared.since = Some(today.clone());
    shared.until = Some(today);
    let today_files = file_groups
        .iter()
        .map(|group| paths::filter_codex_usage_files(&group.dir, &group.files, &shared))
        .collect::<Vec<_>>();
    let today = load_groups_from_file_selection(
        &file_groups,
        &today_files,
        &shared,
        AgentReportKind::Daily,
    )?;
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
