use std::{
    collections::HashSet,
    sync::{Arc, mpsc},
};

use ccusage_core::{LoadedEntry, ReportWindow, TimestampMs, cli::SharedArgs};

pub type NativeSessions = HashSet<Arc<str>>;

pub fn native_sessions(entries: &[(LoadedEntry, Option<Arc<str>>)]) -> NativeSessions {
    entries
        .iter()
        .filter_map(|(_, session)| session.clone())
        .collect()
}

pub struct ClaudeClaimsTx {
    claims: mpsc::SyncSender<ClaudeCounted>,
    requested: mpsc::Receiver<NativeSessions>,
}

pub struct ClaudeClaimsRx {
    claims: mpsc::Receiver<ClaudeCounted>,
    request: mpsc::Sender<NativeSessions>,
}

pub fn channel() -> (ClaudeClaimsTx, ClaudeClaimsRx) {
    let (sender, receiver) = mpsc::sync_channel(1);
    let (request, requested) = mpsc::channel();
    (
        ClaudeClaimsTx {
            claims: sender,
            requested,
        },
        ClaudeClaimsRx {
            claims: receiver,
            request,
        },
    )
}

impl ClaudeClaimsTx {
    pub fn requested(&self) -> Option<NativeSessions> {
        self.requested.recv().ok()
    }

    pub fn publish(self, counted: ClaudeCounted) {
        let _ = self.claims.send(counted);
    }
}

impl ClaudeClaimsRx {
    pub fn receive(self, sessions: NativeSessions) -> ccusage_core::Result<ClaudeCounted> {
        let _ = self.request.send(sessions);
        self.claims
            .recv()
            .map_err(|_| ccusage_core::cli_error("Claude claims producer did not finish"))
    }
}

/// The Claude sessions and report window that cover matching entries in a second source.
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

    pub fn nothing() -> Self {
        Self {
            counted: HashSet::new(),
            window: ReportWindow::unbounded(),
        }
    }

    fn covers(&self, session: &Arc<str>, timestamp: TimestampMs) -> bool {
        self.counted.contains(session) && self.window.contains_timestamp(timestamp)
    }

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
        std::thread::scope(|scope| {
            ccusage_core::parallel::spawn_scoped_worker(scope, move || {
                assert!(sender.requested().is_some());
                sender.publish(super::ClaudeCounted::nothing());
            });
            assert!(receiver.receive(super::NativeSessions::new()).is_ok());
        });
    }

    #[test]
    fn dropping_the_receiver_skips_loading_claims() {
        let (sender, receiver) = super::channel();
        drop(receiver);
        assert!(sender.requested().is_none());
    }

    #[test]
    fn a_failed_producer_unblocks_the_consumer_with_an_error() {
        let (sender, receiver) = super::channel();
        drop(sender);
        assert!(receiver.receive(super::NativeSessions::new()).is_err());
    }
}
