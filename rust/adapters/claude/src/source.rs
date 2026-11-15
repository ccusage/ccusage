use std::collections::BTreeMap;

use ccusage_adapter_common::{
    claims::ClaudeCounted,
    rows::summary_rows,
    source::{ALL_REPORTS, PricingSupply, SourceRows, UnifiedRequest, UsageSource},
};
use ccusage_core::{ReportWindow, Result, SessionAccumulator, UsageSummary, cli::AgentReportKind};

use crate::{load_daily_summaries_with_detection_progress, load_deduped_entries_with_progress};

pub struct Claude;

pub static SOURCE: Claude = Claude;

impl UsageSource for Claude {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn label(&self) -> &'static str {
        "Claude"
    }

    fn reports(&self) -> &'static [AgentReportKind] {
        ALL_REPORTS
    }

    fn pricing_supply(&self) -> PricingSupply {
        PricingSupply::Source
    }

    fn unified_rows(&self, request: &UnifiedRequest<'_>) -> Result<SourceRows> {
        let shared = request.shared;
        let window = ReportWindow::from_shared(shared);
        let project_filter = request.options.claude_project;

        if request.kind == AgentReportKind::Session {
            let deduped =
                load_deduped_entries_with_progress(shared, project_filter, request.progress)?;
            let detected = !deduped.entries.is_empty();
            let sessions = deduped
                .main_thread_session_days(|entry| window.contains_date(&entry.date))
                .into_iter()
                .map(|(session, _)| session)
                .collect();
            let entries = deduped
                .entries
                .into_iter()
                .filter(|entry| window.contains_date(&entry.date))
                .collect::<Vec<_>>();
            let summaries = summarize_entry_sessions(&entries)?;

            return Ok(SourceRows {
                rows: summary_rows("claude", summaries, false),
                detected,
                warnings: Vec::new(),
                claude_counted: Some(ClaudeCounted::sessions(sessions, shared)),
            });
        }

        let loaded = load_daily_summaries_with_detection_progress(
            shared,
            project_filter,
            false,
            request.progress,
        )?;
        let summaries = loaded
            .summaries
            .into_iter()
            .filter(|summary| {
                window.is_unbounded()
                    || window.contains_date(summary.date.as_deref().unwrap_or_default())
            })
            .collect();
        let session_days = loaded
            .session_days
            .into_iter()
            .filter(|(_, date)| window.contains_date(date))
            .collect();

        Ok(SourceRows {
            rows: summary_rows("claude", summaries, false),
            detected: loaded.detected,
            warnings: Vec::new(),
            claude_counted: Some(ClaudeCounted::session_days(session_days, shared)),
        })
    }
}

fn summarize_entry_sessions(entries: &[ccusage_core::LoadedEntry]) -> Result<Vec<UsageSummary>> {
    let mut groups = BTreeMap::<(String, String), SessionAccumulator>::new();
    for entry in entries {
        groups
            .entry((entry.project_path.to_string(), entry.session_id.to_string()))
            .or_default()
            .add_entry(entry);
    }
    groups
        .into_values()
        .map(|group| group.into_summary())
        .collect()
}
