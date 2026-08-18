use ccusage_adapter_common::{
    collect_files_with_extension, filter_loaded_entries_by_date, read_files_parallel,
};
use ccusage_core::*;

mod loader;
mod parser;
mod paths;
mod report;

use crate::{
    Result, cli::AgentCommandArgs, print_json_or_jq, print_usage_table, sort_summaries, wants_json,
};

#[doc(hidden)]
#[derive(Debug)]
pub struct PiEntry {
    pub provider: Option<String>,
    pub entry: LoadedEntry,
}

impl std::ops::Deref for PiEntry {
    type Target = LoadedEntry;

    fn deref(&self) -> &Self::Target {
        &self.entry
    }
}

pub use loader::{load_entries, load_entries_for_store_paths};
#[doc(hidden)]
pub use loader::{
    load_entries_for_store_path, load_entries_for_store_paths_with_provider,
    load_entries_with_provider,
};
pub use paths::{named_store_paths, paths as default_paths};
pub use report::{report_from_rows, summarize_entries};

pub fn run(args: AgentCommandArgs) -> Result<()> {
    let pricing = crate::PricingMap::load_with_overrides(
        args.shared.offline,
        crate::log_level() != Some(0),
        args.shared.pricing_overrides.iter(),
    );
    let mut entries = load_entries(&args.shared, args.pi_path.as_deref(), Some(&pricing))?;
    filter_loaded_entries_by_date(&mut entries, &args.shared);
    let mut rows = summarize_entries(&entries, args.kind)?;
    sort_summaries(&mut rows, &args.shared.order, |row| {
        ccusage_core::summary_period(row)
    });
    if wants_json(&args.shared) {
        return print_json_or_jq(
            report_from_rows(&rows, args.kind),
            args.shared.jq.as_deref(),
            args.shared.no_cost,
        );
    }
    print_usage_table(
        "pi-agent Token Usage Report",
        ccusage_core::first_column(args.kind),
        &rows,
        &args.shared,
        false,
        None,
    )?;
    Ok(())
}
