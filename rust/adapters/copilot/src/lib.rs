use ccusage_adapter_common::{
    collect_files_with_extension, filter_loaded_entries_by_date, read_files_parallel,
};
use ccusage_core::*;

mod loader;
mod parser;
mod paths;
mod report;

use crate::cli::AgentCommandArgs;
use crate::{PricingMap, Result, print_json_or_jq, print_usage_table, sort_summaries, wants_json};

pub use loader::load_entries;
pub(crate) use report::report_from_rows;
pub use report::summarize_entries;

pub fn run(args: AgentCommandArgs) -> Result<()> {
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
        ccusage_core::summary_period(row)
    });
    if wants_json(&shared) {
        return print_json_or_jq(
            report_from_rows(&rows, args.kind),
            shared.jq.as_deref(),
            shared.no_cost,
        );
    }
    if rows.is_empty() {
        eprintln!("{}", empty_usage_message());
        return Ok(());
    }
    print_usage_table(
        "GitHub Copilot CLI Token Usage Report",
        ccusage_core::first_column(args.kind),
        &rows,
        &shared,
        false,
        None,
    )?;
    Ok(())
}

fn empty_usage_message() -> &'static str {
    "No GitHub Copilot CLI usage data found.\nEnable Copilot OpenTelemetry file export before starting or resuming Copilot sessions.\nSee https://ccusage.com/guide/copilot/#data-source"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_usage_message_links_to_copilot_docs() {
        let message = empty_usage_message();
        assert!(message.contains("https://ccusage.com/guide/copilot/#data-source"));
    }
}
