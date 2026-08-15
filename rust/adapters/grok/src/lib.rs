use ccusage_adapter_common::{
    collect_files_with_extension, filter_loaded_entries_by_date, read_files_parallel,
};
use ccusage_core::*;

mod loader;
mod parser;
mod paths;
mod report;

use crate::{
    PricingMap, Result, UsageTableOptions, cli::AgentCommandArgs, print_json_or_jq,
    print_usage_table_with_options, sort_summaries, wants_json,
};

pub use loader::{has_data, load_entries};
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
    let table_options = table_options(&rows);
    print_usage_table_with_options(
        "Grok Token Usage Report",
        ccusage_core::first_column(args.kind),
        &rows,
        &shared,
        false,
        None,
        table_options,
    )?;
    Ok(())
}

fn table_options(rows: &[UsageSummary]) -> UsageTableOptions {
    UsageTableOptions {
        show_cache_creation: rows.iter().any(|row| row.cache_creation_tokens > 0),
    }
}

#[cfg(test)]
mod smoke_tests {
    use super::*;
    use crate::cli::SharedArgs;

    #[test]
    #[ignore = "requires real Grok home with sessions/**/updates.jsonl"]
    fn smoke_real_grok_home_loads_without_error() {
        let shared = SharedArgs {
            timezone: Some("UTC".into()),
            ..SharedArgs::default()
        };
        let pricing = PricingMap::load_embedded();
        let entries = load_entries(&shared, &pricing).unwrap();
        let _ = entries.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hides_cache_creation_when_selected_rows_are_zero() {
        assert!(!table_options(&[summary(0), summary(0)]).show_cache_creation);
    }

    #[test]
    fn shows_cache_creation_when_any_selected_row_is_nonzero() {
        assert!(table_options(&[summary(0), summary(25)]).show_cache_creation);
    }

    fn summary(cache_creation_tokens: u64) -> UsageSummary {
        UsageSummary {
            date: Some("2026-08-15".to_string()),
            month: None,
            week: None,
            session_id: None,
            project_path: None,
            last_activity: None,
            first_activity: None,
            input_tokens: 100,
            output_tokens: 20,
            cache_creation_tokens,
            cache_read_tokens: 40,
            extra_total_tokens: 0,
            total_cost: 0.01,
            credits: None,
            message_count: None,
            models_used: vec!["grok-4.5-build".to_string()],
            model_breakdowns: Vec::new(),
            project: None,
            versions: None,
        }
    }
}
