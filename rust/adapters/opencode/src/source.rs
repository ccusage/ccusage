use ccusage_adapter_common::{
    claims::ClaudeCounted,
    rows::summary_rows,
    source::{ALL_REPORTS, PricingSupply, ProgressOwner, SourceRows, UnifiedRequest, UsageSource},
};
use ccusage_core::{
    ReportWindow, Result,
    cli::AgentReportKind,
    progress::{UsageLoadAgent, track_usage_load},
};

use crate::{has_data, load_entries_with_native_sessions_with_progress, summarize_entries};

pub struct OpenCode;

pub static SOURCE: OpenCode = OpenCode;

impl UsageSource for OpenCode {
    fn id(&self) -> &'static str {
        "opencode"
    }

    fn label(&self) -> &'static str {
        "OpenCode"
    }

    fn reports(&self) -> &'static [AgentReportKind] {
        ALL_REPORTS
    }

    fn pricing_supply(&self) -> PricingSupply {
        PricingSupply::Source
    }

    fn unified_rows(&self, request: &UnifiedRequest<'_>) -> Result<SourceRows> {
        let load = || unified_rows(request);
        match request.progress {
            ProgressOwner::Source => {
                track_usage_load(UsageLoadAgent("OpenCode"), request.shared.json, load)
            }
            ProgressOwner::Caller => load(),
        }
    }
}

fn unified_rows(request: &UnifiedRequest<'_>) -> Result<SourceRows> {
    let linked = load_entries_with_native_sessions_with_progress(
        request.shared,
        request.kind,
        ProgressOwner::Caller,
    )?;
    let detected = !linked.is_empty() || has_data();
    let mut entries = if linked.iter().any(|(_, session)| session.is_some()) {
        request
            .claude_counted
            .map_or_else(ClaudeCounted::nothing, |load| load())
            .retain_uncounted(linked)
    } else {
        linked.into_iter().map(|(entry, _)| entry).collect()
    };
    let window = ReportWindow::from_shared(request.shared);
    entries.retain(|entry| window.contains_date(&entry.date));
    let summaries = summarize_entries(&entries, request.kind)?;

    Ok(SourceRows {
        rows: summary_rows("opencode", summaries, false),
        detected,
        warnings: Vec::new(),
        claude_counted: None,
    })
}
