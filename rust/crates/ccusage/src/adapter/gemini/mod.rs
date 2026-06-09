mod loader;
mod parser;
mod paths;
mod report;

use crate::{
    PricingMap, Result, adapter::opencode, cli::AgentCommandArgs, filter_loaded_entries_by_date,
    print_json_or_jq, print_usage_table, sort_summaries, wants_json,
};

pub(crate) use loader::load_entries;
pub(crate) use report::{report_from_rows, summarize_entries};

#[cfg(test)]
static GEMINI_DATA_DIR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
struct GeminiDataDirEnvGuard {
    _guard: ccusage_test_support::EnvVarGuard,
}

#[cfg(test)]
impl GeminiDataDirEnvGuard {
    fn set(path: &std::path::Path) -> Self {
        Self {
            _guard: ccusage_test_support::EnvVarGuard::set(paths::GEMINI_DATA_DIR_ENV, path),
        }
    }
}

pub(crate) fn run(args: AgentCommandArgs) -> Result<()> {
    let shared = args.shared;
    let pricing = PricingMap::load_with_overrides(
        shared.offline,
        crate::log_level() != Some(0),
        shared.pricing_overrides.iter(),
    );
    let mut entries = load_entries(&shared, &pricing)?;
    filter_loaded_entries_by_date(&mut entries, &shared);
    let mut rows = summarize_entries(&entries, args.kind)?;
    sort_summaries(&mut rows, &shared.order, |row| {
        opencode::summary_period(row)
    });
    if wants_json(&shared) {
        return print_json_or_jq(
            report_from_rows(&rows, args.kind),
            shared.jq.as_deref(),
            shared.no_cost,
        );
    }
    print_usage_table(
        "Gemini CLI Token Usage Report",
        opencode::first_column(args.kind),
        &rows,
        &shared,
        false,
        None,
    )?;
    Ok(())
}
