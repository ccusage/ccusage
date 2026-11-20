use ccusage_core::Diagnostics;
use std::{path::PathBuf, sync::Arc};

use crate::{
    Result, Speed, TokenUsageRaw, UsageSummary,
    cli::{IsoDate, SharedArgs},
    fast::suffix_string,
};

use super::{
    MainThreadSessions, UsageLoadContext,
    dedupe::{DedupEntry, DedupKey, dedupe_files},
    main_thread_sessions,
    paths::{claude_paths, read_usage_files_since, visit_usage_files},
    read_discovered_paths,
};

mod accumulate;
mod read;
mod record;
mod response_chain;

use accumulate::summarize_daily_entries;
use read::read_daily_usage_file_with_diagnostics;
pub(super) use read::{has_usage, read_usage_file};
pub(super) use record::parse_usage_line;
use record::{ClaudeEntry, RecordedCost};

pub(super) struct DailyEntries {
    deduped: DedupedDailyEntries,
    pub(super) detected: bool,
}

impl DailyEntries {
    pub(super) fn summaries(
        &self,
        group_by_project: bool,
        window: Option<&ccusage_core::ReportWindow>,
    ) -> Vec<UsageSummary> {
        summarize_daily_entries(&self.deduped.entries, group_by_project, window)
    }

    pub(super) fn sessions(&self) -> MainThreadSessions {
        sessions(&self.deduped)
    }
}

pub(super) fn load_daily_entries(
    shared: &SharedArgs,
    project_filter: Option<&str>,
    diagnostics: &Diagnostics,
) -> Result<DailyEntries> {
    let paths = claude_paths()?;
    let context = UsageLoadContext::new(shared);
    let mut pruned = Vec::new();
    let read = |file: &PathBuf| {
        read_daily_usage_file_with_diagnostics(
            file,
            context.window().timezone(),
            context.table(),
            diagnostics,
        )
    };
    let loaded_files = if shared.since.is_some() {
        let since_files = read_usage_files_since(
            &paths,
            project_filter,
            diagnostics,
            shared,
            shared.clock.now().into(),
            read,
        );
        pruned = since_files.pruned;
        since_files.kept
    } else {
        read_discovered_paths(
            shared.single_thread,
            |visit| visit_usage_files(&paths, project_filter, visit, diagnostics),
            read,
        )
    };
    let deduped = (!loaded_files.is_empty()).then(|| {
        let (entries, copies) = dedupe_files(loaded_files);
        DedupedDailyEntries { entries, copies }
    });
    let mut deduped = deduped.unwrap_or(DedupedDailyEntries {
        entries: Vec::new(),
        copies: Vec::new(),
    });
    let detected = !deduped.entries.is_empty() || pruned.iter().any(|file| has_usage(file));
    deduped.retain_entries_in(&ccusage_core::ReportWindow::from_shared(shared));
    Ok(DailyEntries { deduped, detected })
}

pub(super) struct DedupedDailyEntries {
    entries: Vec<ClaudeEntry>,
    copies: Vec<(usize, Arc<str>)>,
}

impl DedupedDailyEntries {
    fn retain_entries_in(&mut self, window: &ccusage_core::ReportWindow) {
        super::retain_copied_entries(&mut self.entries, &mut self.copies, window, |entry| {
            entry.date
        });
    }
}

fn sessions(deduped: &DedupedDailyEntries) -> MainThreadSessions {
    main_thread_sessions(
        deduped
            .entries
            .iter()
            .map(|entry| (&entry.key.session_id, entry))
            .chain(
                deduped
                    .copies
                    .iter()
                    .map(|(index, session_id)| (session_id, &deduped.entries[*index])),
            )
            .map(|(session_id, survivor)| (session_id.as_ref(), survivor.is_sidechain())),
    )
}

pub(super) struct ParsedUsage {
    pub(super) key: DedupKey,
    pub(super) date: IsoDate,
    pub(super) project: Arc<str>,
    pub(super) usage: TokenUsageRaw,
    pub(super) cost: f64,
    pub(super) missing_pricing_model: Option<String>,
    pub(super) stored_cost: Option<f64>,
    pub(super) raw_model: Option<String>,
    pub(super) timestamp_text: String,
    pub(super) version: Option<String>,
    pub(super) recorded_session_id: Option<String>,
    pub(super) cost_usd: Option<f64>,
    pub(super) is_api_error_message: Option<bool>,
}

impl ParsedUsage {
    pub(super) fn display_model(
        raw_model: Option<String>,
        usage: &TokenUsageRaw,
    ) -> Option<String> {
        raw_model.and_then(|model| {
            if model == "<synthetic>" {
                None
            } else if matches!(usage.speed, Some(Speed::Fast)) {
                Some(suffix_string(&model, "-fast"))
            } else {
                Some(model)
            }
        })
    }

    fn into_daily(self) -> ClaudeEntry {
        ClaudeEntry {
            model: Self::display_model(self.raw_model, &self.usage),
            key: self.key,
            date: self.date,
            project: self.project,
            usage: self.usage,
            cost: self.cost,
            missing_pricing_model: self.missing_pricing_model,
            recorded_cost: RecordedCost::new(self.cost_usd, self.stored_cost),
        }
    }
}

#[cfg(test)]
mod tests;
