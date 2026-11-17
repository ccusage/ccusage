use std::{
    collections::HashSet,
    sync::{Arc, mpsc},
};

use ccusage_core::{LoadedEntry, ReportWindow, TimestampMs, cli::SharedArgs};

pub struct ClaudeClaimsTx(mpsc::SyncSender<ClaudeCounted>);
pub struct ClaudeClaimsRx(mpsc::Receiver<ClaudeCounted>);

pub fn channel() -> (ClaudeClaimsTx, ClaudeClaimsRx) {
    let (sender, receiver) = mpsc::sync_channel(1);
    (ClaudeClaimsTx(sender), ClaudeClaimsRx(receiver))
}

impl ClaudeClaimsTx {
    pub fn publish(self, counted: ClaudeCounted) {
        let _ = self.0.send(counted);
    }
}

impl ClaudeClaimsRx {
    pub fn receive(self) -> ccusage_core::Result<ClaudeCounted> {
        self.0
            .recv()
            .map_err(|_| ccusage_core::cli_error("Claude claims producer did not finish"))
    }
}

/// The Claude Code usage the Claude source counts in one report, so a source
/// that runs Claude Code, such as OpenCode's `claude-agent` provider, can skip
/// its copy.
pub struct ClaudeCounted {
    counted: HashSet<Arc<str>>,
    window: ReportWindow,
}

impl ClaudeCounted {
    pub fn sessions(sessions: HashSet<Arc<str>>, claude: &SharedArgs) -> Self {
        Self {
            counted: sessions,
            window: ReportWindow::from_shared(claude),
        }
    }

    /// What a Claude source that did not load counted.
    pub fn nothing() -> Self {
        Self {
            counted: HashSet::new(),
            window: ReportWindow::unbounded(),
        }
    }

    fn covers(&self, session: &Arc<str>, timestamp: TimestampMs) -> bool {
        self.counted.contains(session) && self.window.contains_timestamp(timestamp)
    }

    /// Drops the entries linked to a Claude Code session the Claude source
    /// counted, so each turn is counted once, priced by the resolved model the
    /// Claude transcript records. OpenCode stores only the usage of turns
    /// Claude Code also writes to that session's transcript, so a counted
    /// session holds every turn of its OpenCode rows.
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

#[cfg(test)]
mod channel_tests {
    #[test]
    fn publication_reaches_the_receiver() {
        let (sender, receiver) = super::channel();
        sender.publish(super::ClaudeCounted::nothing());
        assert!(receiver.receive().is_ok());
    }

    #[test]
    fn a_failed_producer_unblocks_the_consumer_with_an_error() {
        let (sender, receiver) = super::channel();
        drop(sender);
        assert!(receiver.receive().is_err());
    }
}
