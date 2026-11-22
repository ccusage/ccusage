use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use ccusage_core::{
    LoadedEntry, Result, Skip, SkipKind, TokenUsageRaw, cli::SharedArgs, date_at,
    parse_ts_timestamp, resolve_tz,
};

use crate::source::{EntryDescriptor, EntrySource, LoadContext};

pub(crate) struct Fake {
    pub(crate) descriptor: EntryDescriptor,
    pub(crate) entries: Vec<LoadedEntry>,
    pub(crate) skips_a_unit: bool,
    pub(crate) saw_pricing: AtomicBool,
}

impl Fake {
    pub(crate) fn new(descriptor: EntryDescriptor, entries: Vec<LoadedEntry>) -> Self {
        Self {
            descriptor,
            entries,
            skips_a_unit: false,
            saw_pricing: AtomicBool::new(false),
        }
    }
}

impl EntrySource for Fake {
    fn descriptor(&self) -> EntryDescriptor {
        self.descriptor
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
        self.saw_pricing
            .store(cx.pricing.get().is_some(), Ordering::Relaxed);
        if self.skips_a_unit {
            cx.diagnostics.record(Skip::unit(
                Path::new("/data/broken.json"),
                SkipKind::Unreadable,
                "permission denied",
            ));
        }
        Ok(self.entries.clone())
    }
}

pub(crate) fn descriptor() -> EntryDescriptor {
    EntryDescriptor {
        session: crate::source::SessionShape::WithActivity,
        ..EntryDescriptor::standard("fake", "Fake")
    }
}

pub(crate) fn entry(session: &str, timestamp: &str, input_tokens: u64) -> LoadedEntry {
    let parsed = parse_ts_timestamp(timestamp).unwrap();
    LoadedEntry {
        usage: TokenUsageRaw {
            input_tokens,
            ..TokenUsageRaw::default()
        },
        message_id: None,
        version: None,
        recorded_cost: None,
        timestamp: parsed,
        date: date_at(
            parsed,
            &resolve_tz(Some(&ccusage_core::cli::ReportTimezone::UTC)),
        ),
        project: Arc::from("fake"),
        session_id: Arc::from(session),
        project_path: Arc::from("project"),
        cost: 0.0,
        extra_total_tokens: 0,
        credits: None,
        message_count: None,
        model: Some("model".to_string()),
        usage_limit_reset_time: None,
        missing_pricing_model: None,
        estimated_pricing: false,
        stored_cost: None,
    }
}

pub(crate) fn entries() -> Vec<LoadedEntry> {
    vec![
        entry("early", "2026-01-01T10:00:00.000Z", 1),
        entry("long", "2026-01-01T12:00:00.000Z", 10),
        entry("long", "2026-01-02T12:00:00.000Z", 100),
        entry("long", "2026-01-03T12:00:00.000Z", 1000),
    ]
}

pub(crate) fn shared(since: Option<&str>, until: Option<&str>) -> SharedArgs {
    ccusage_test_support::install_pricing_snapshots();
    SharedArgs {
        since: since.map(|date| date.parse().unwrap()),
        until: until.map(|date| date.parse().unwrap()),
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        offline: true,
        ..SharedArgs::with_defaults()
    }
}
