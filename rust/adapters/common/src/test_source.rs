//! A stand-in source for the tests of the pipeline and of the parity check.

use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use serde_json::{Value, json};

use ccusage_core::{
    LoadedEntry, Result, SessionAccumulator, Skip, SkipKind, TokenUsageRaw, UsageEntry,
    UsageMessage, UsageSummary,
    cli::{AgentReportKind, SharedArgs},
    format_date_tz, parse_ts_timestamp, parse_tz, summarize_by_key, totals_json,
};

use crate::source::{EntryDescriptor, EntrySource, LoadContext, PricingSupply};

pub(crate) struct Fake {
    pub(crate) descriptor: EntryDescriptor,
    pub(crate) entries: Vec<LoadedEntry>,
    pub(crate) skips_a_unit: bool,
    /// Makes the unified summary lose the first entry, as a drifted adapter would.
    pub(crate) unified_drops_an_entry: bool,
    pub(crate) saw_pricing: AtomicBool,
}

impl Fake {
    pub(crate) fn new(descriptor: EntryDescriptor, entries: Vec<LoadedEntry>) -> Self {
        Self {
            descriptor,
            entries,
            skips_a_unit: false,
            unified_drops_an_entry: false,
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
            .store(cx.pricing.is_some(), Ordering::Relaxed);
        if self.skips_a_unit {
            cx.diagnostics.record(Skip::unit(
                Path::new("/data/broken.json"),
                SkipKind::Unreadable,
                "permission denied",
            ));
        }
        Ok(self.entries.clone())
    }

    fn summarize(
        &self,
        entries: &[LoadedEntry],
        kind: AgentReportKind,
    ) -> Result<Vec<UsageSummary>> {
        if kind == AgentReportKind::Session {
            let mut groups = BTreeMap::<String, SessionAccumulator>::new();
            for entry in entries {
                groups
                    .entry(entry.session_id.to_string())
                    .or_default()
                    .add_entry(entry);
            }
            return groups
                .into_values()
                .map(SessionAccumulator::into_summary)
                .collect();
        }
        summarize_by_key(
            entries,
            |entry| entry.date.clone(),
            |date| (date.to_string(), None),
        )
    }

    fn summarize_unified(
        &self,
        entries: &[LoadedEntry],
        kind: AgentReportKind,
    ) -> Result<Vec<UsageSummary>> {
        let skipped = usize::from(self.unified_drops_an_entry).min(entries.len());
        self.summarize(&entries[skipped..], kind)
    }

    fn report_json(&self, rows: &[UsageSummary], _kind: AgentReportKind) -> Value {
        json!({ "rows": rows.len(), "totals": totals_json(rows) })
    }

    fn print_table(
        &self,
        _rows: &[UsageSummary],
        _kind: AgentReportKind,
        _shared: &SharedArgs,
    ) -> Result<()> {
        Ok(())
    }
}

pub(crate) fn descriptor() -> EntryDescriptor {
    EntryDescriptor {
        pricing: PricingSupply::Source,
        ..EntryDescriptor::standard("fake", "Fake")
    }
}

pub(crate) fn entry(session: &str, timestamp: &str, input_tokens: u64) -> LoadedEntry {
    let parsed = parse_ts_timestamp(timestamp).unwrap();
    LoadedEntry {
        data: UsageEntry {
            session_id: Some(session.to_string()),
            timestamp: timestamp.to_string(),
            version: None,
            message: UsageMessage {
                usage: TokenUsageRaw {
                    input_tokens,
                    ..TokenUsageRaw::default()
                },
                model: Some("model".to_string()),
                id: None,
            },
            cost_usd: None,
            request_id: None,
            is_api_error_message: None,
            is_sidechain: None,
        },
        timestamp: parsed,
        date: format_date_tz(parsed, parse_tz(Some("UTC")).as_ref()),
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
        stored_cost: None,
    }
}

/// Session `long` runs across three days and `early` ends before them.
pub(crate) fn entries() -> Vec<LoadedEntry> {
    vec![
        entry("early", "2026-01-01T10:00:00.000Z", 1),
        entry("long", "2026-01-01T12:00:00.000Z", 10),
        entry("long", "2026-01-02T12:00:00.000Z", 100),
        entry("long", "2026-01-03T12:00:00.000Z", 1000),
    ]
}

pub(crate) fn shared(since: Option<&str>, until: Option<&str>) -> SharedArgs {
    SharedArgs {
        since: since.map(str::to_string),
        until: until.map(str::to_string),
        timezone: Some("UTC".to_string()),
        offline: true,
        ..SharedArgs::with_defaults()
    }
}
