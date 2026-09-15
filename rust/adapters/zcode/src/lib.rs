use ccusage_adapter_common::filter_loaded_entries_by_date;
use ccusage_core::*;

mod loader;
mod parser;
mod paths;
mod report;

use crate::{
    PricingMap, Result, UsageTableOptions, cli::AgentCommandArgs, print_json_or_jq,
    print_usage_table_with_options, sort_summaries, wants_json,
};

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
    let table_options = UsageTableOptions {
        show_cache_creation: rows.iter().any(|row| row.cache_creation_tokens > 0),
    };
    print_usage_table_with_options(
        "ZCode Token Usage Report",
        ccusage_core::first_column(args.kind),
        &rows,
        &shared,
        false,
        None,
        table_options,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_the_standard_cache_creation_table_option() {
        let rows = [UsageSummary {
            date: Some("2026-08-16".to_string()),
            month: None,
            week: None,
            session_id: None,
            project_path: None,
            last_activity: None,
            first_activity: None,
            input_tokens: 1,
            output_tokens: 1,
            cache_creation_tokens: 2,
            cache_read_tokens: 3,
            extra_total_tokens: 0,
            total_cost: 0.0,
            credits: None,
            message_count: None,
            models_used: vec!["GLM-5.3".to_string()],
            model_breakdowns: Vec::new(),
            project: None,
            versions: None,
        }];

        assert!(rows.iter().any(|row| row.cache_creation_tokens > 0));
    }
}
