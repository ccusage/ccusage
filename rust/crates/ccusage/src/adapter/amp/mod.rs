mod loader;
mod parser;
mod paths;
mod report;
mod server;

use crate::{
    PricingMap, Result, adapter::opencode, cli::AgentCommandArgs, filter_loaded_entries_by_date,
    print_json_or_jq, sort_summaries, wants_json,
};

pub(crate) use loader::load_entries;
#[cfg(test)]
pub(crate) use parser::read_thread_file;
pub(crate) use report::{print_table_for_agent, report_from_rows, summarize_entries};

pub(crate) fn run(args: AgentCommandArgs) -> Result<()> {
    let shared = args.shared;
    let pricing = PricingMap::load_with_overrides(
        shared.offline,
        crate::log_level() != Some(0),
        shared.pricing_overrides.iter(),
    );
    let mut entries = loader::load_entries_for_amp_command(&shared, &pricing)?;
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
    report::print_table(args.kind, &rows, &shared)
}
