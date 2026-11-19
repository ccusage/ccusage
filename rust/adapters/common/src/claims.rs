use std::{collections::HashSet, sync::Arc};

use ccusage_core::{LoadedEntry, ReportWindow, TimestampMs, cli::SharedArgs, format_date_tz};

/// The Claude Code usage the Claude source counts in one report, so a source
/// that runs Claude Code, such as OpenCode's `claude-agent` provider, can skip
/// its copy.
pub struct ClaudeCounted {
    counted: Counted,
    /// The window and timezone the Claude source loaded with, which can differ
    /// from the other source's under per-source configuration.
    window: ReportWindow,
}

enum Counted {
    /// Daily-family reports count usage on the day Claude logged it.
    SessionDays(HashSet<(Arc<str>, String)>),
    /// A session report counts every session with in-window usage.
    Sessions(HashSet<Arc<str>>),
}

impl ClaudeCounted {
    /// The session days a daily-family report counted. `claude` is the options
    /// the Claude source loaded with.
    pub fn session_days(days: HashSet<(Arc<str>, String)>, claude: &SharedArgs) -> Self {
        Self {
            counted: Counted::SessionDays(days),
            window: ReportWindow::from_shared(claude),
        }
    }

    /// The sessions a session report counted. `claude` is the options the
    /// Claude source loaded with.
    pub fn sessions(sessions: HashSet<Arc<str>>, claude: &SharedArgs) -> Self {
        Self {
            counted: Counted::Sessions(sessions),
            window: ReportWindow::from_shared(claude),
        }
    }

    /// What a Claude source that did not load counted.
    pub fn nothing() -> Self {
        Self {
            counted: Counted::Sessions(HashSet::new()),
            window: ReportWindow::unbounded(),
        }
    }

    fn covers(&self, session: &Arc<str>, timestamp: TimestampMs) -> bool {
        let date = || format_date_tz(timestamp, Some(self.window.timezone()));
        match &self.counted {
            Counted::SessionDays(days) => days.contains(&(Arc::clone(session), date())),
            // Claude counts only the session's in-window days, so a row outside them stays.
            Counted::Sessions(sessions) => {
                sessions.contains(session) && self.window.contains_date(&date())
            }
        }
    }

    /// Drops the entries linked to a Claude Code session the Claude source
    /// counted, so each turn is counted once, priced by the resolved model the
    /// Claude transcript records. OpenCode stores only the usage of turns
    /// Claude Code also writes to that session's transcript, so a counted
    /// session holds every turn of its OpenCode rows. Daily-family reports
    /// match per day, and a row is dated by its turn's start, so a turn whose
    /// calls Claude logs after midnight lands on a day Claude may not have
    /// counted and is kept.
    pub fn retain_uncounted(
        &self,
        entries: Vec<(LoadedEntry, Option<Arc<str>>)>,
    ) -> Vec<LoadedEntry> {
        entries
            .into_iter()
            .filter(|(entry, native_session)| {
                native_session
                    .as_ref()
                    .is_none_or(|session| !self.covers(session, entry.timestamp))
            })
            .map(|(entry, _)| entry)
            .collect()
    }
}
