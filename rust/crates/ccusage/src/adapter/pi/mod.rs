mod loader;
mod parser;
mod paths;
mod report;

use crate::{
    Result, cli::AgentCommandArgs, filter_loaded_entries_by_date, print_json_or_jq,
    print_usage_table, sort_summaries, wants_json,
};

#[cfg(test)]
pub(crate) use loader::load_entries_for_store_path;
pub(crate) use loader::{load_entries, load_entries_for_store_paths};
pub(crate) use paths::{named_store_paths, paths as default_paths};
pub(crate) use report::{report_from_rows, summarize_entries};

pub(crate) fn run(args: AgentCommandArgs) -> Result<()> {
    let pricing = crate::PricingMap::load_with_overrides(
        args.shared.offline,
        crate::log_level() != Some(0),
        args.shared.pricing_overrides.iter(),
    );
    let mut entries = load_entries(&args.shared, args.pi_path.as_deref(), Some(&pricing))?;
    filter_loaded_entries_by_date(&mut entries, &args.shared);
    let mut rows = summarize_entries(&entries, args.kind)?;
    sort_summaries(&mut rows, &args.shared.order, |row| {
        super::opencode::summary_period(row)
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
        super::opencode::first_column(args.kind),
        &rows,
        &args.shared,
        false,
        None,
    )?;
    Ok(())
}
