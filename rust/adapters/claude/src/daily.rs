use ccusage_core::Diagnostics;
use std::{borrow::Cow, path::PathBuf, sync::Arc};

use crate::{
    Result, Speed, TokenUsageRaw, UsageSummary,
    cli::{IsoDate, SharedArgs},
    fast::suffix_string,
};

use super::{
    MainThreadSessions, UsageLoadContext, containing_projects_root,
    dedupe::{DedupKey, dedupe_files},
    main_thread_sessions,
    mirror_claims::read_claiming_files,
    paths::{claude_paths, projects_roots, read_usage_files_since, visit_usage_files},
    read_discovered_paths,
};

mod accumulate;
mod read;
mod record;
mod response_chain;

use accumulate::summarize_daily_entries;
use read::read_daily_usage_file_with_diagnostics;
pub(super) use read::{
    ClaimEntry, find_mirror_sessions, has_usage, read_claim_entries, read_usage_file,
};
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
    let projects_roots = projects_roots(&paths);
    let mut pruned = Vec::new();
    let read = |file: &PathBuf| {
        read_daily_usage_file_with_diagnostics(
            file,
            containing_projects_root(file, &projects_roots),
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
    Ok(finish_daily_files(shared, loaded_files, || {
        pruned.iter().any(|file| has_usage(file))
    }))
}

pub(super) fn load_claiming_daily_entries(
    shared: &SharedArgs,
    project: &str,
    diagnostics: &Diagnostics,
) -> Result<(DailyEntries, MainThreadSessions)> {
    let context = UsageLoadContext::new(shared);
    let loaded = read_claiming_files(shared, project, diagnostics, |file, root| {
        read_daily_usage_file_with_diagnostics(
            file,
            root,
            context.window().timezone(),
            context.table(),
            diagnostics,
        )
    })?;
    Ok((
        finish_daily_files(shared, loaded.report, || loaded.pruned_usage),
        loaded.sessions,
    ))
}

fn finish_daily_files(
    shared: &SharedArgs,
    files: Vec<(PathBuf, Vec<ClaudeEntry>)>,
    pruned_usage: impl FnOnce() -> bool,
) -> DailyEntries {
    let deduped = (!files.is_empty()).then(|| {
        let (entries, copies) = dedupe_files(files);
        DedupedDailyEntries { entries, copies }
    });
    let mut deduped = deduped.unwrap_or(DedupedDailyEntries {
        entries: Vec::new(),
        copies: Vec::new(),
    });
    let detected = !deduped.entries.is_empty() || pruned_usage();
    deduped.retain_entries_in(&ccusage_core::ReportWindow::from_shared(shared));
    DailyEntries { deduped, detected }
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
    main_thread_sessions(&deduped.entries, &deduped.copies)
}

pub(super) struct ParsedUsage<'a> {
    pub(super) key: DedupKey,
    pub(super) is_sidechain: bool,
    pub(super) date: IsoDate,
    pub(super) project: Arc<str>,
    pub(super) usage: TokenUsageRaw,
    pub(super) cost: f64,
    pub(super) missing_pricing_model: Option<String>,
    pub(super) estimated_pricing: bool,
    pub(super) stored_cost: Option<f64>,
    pub(super) raw_model: Option<String>,
    pub(super) timestamp_text: Cow<'a, str>,
    pub(super) version: Option<Cow<'a, str>>,
    pub(super) recorded_session_id: Option<Arc<str>>,
    pub(super) cost_usd: Option<f64>,
    pub(super) is_api_error_message: Option<bool>,
}

impl ParsedUsage<'_> {
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

    fn into_daily(self, models: &mut DisplayModels) -> ClaudeEntry {
        ClaudeEntry {
            model: models.intern(self.raw_model.as_deref(), &self.usage),
            key: self.key,
            is_sidechain: self.is_sidechain,
            date: self.date,
            project: self.project,
            usage: self.usage,
            cost: self.cost,
            missing_pricing: self.missing_pricing_model.is_some(),
            estimated_pricing: self.estimated_pricing,
            recorded_cost: RecordedCost::new(self.cost_usd, self.stored_cost),
        }
    }
}

struct DisplayModel {
    raw: Box<str>,
    fast: bool,
    display: Option<Arc<str>>,
}

#[derive(Default)]
struct DisplayModels(smallvec::SmallVec<[DisplayModel; 2]>);

impl DisplayModels {
    fn intern(&mut self, raw_model: Option<&str>, usage: &TokenUsageRaw) -> Option<Arc<str>> {
        let raw_model = raw_model?;
        let fast = matches!(usage.speed, Some(Speed::Fast));
        if let Some(known) = self
            .0
            .iter()
            .find(|known| known.fast == fast && known.raw.as_ref() == raw_model)
        {
            return known.display.clone();
        }
        let display =
            ParsedUsage::display_model(Some(raw_model.to_owned()), usage).map(Arc::<str>::from);
        self.0.push(DisplayModel {
            raw: raw_model.into(),
            fast,
            display: display.clone(),
        });
        display
    }
}

#[cfg(test)]
mod tests;
