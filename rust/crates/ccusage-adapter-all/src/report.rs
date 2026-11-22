use crate::{Totals, TotalsRow, cli::SortOrder};

use super::types::ReportRow;

mod json;
mod table;

#[cfg(test)]
pub(super) use json::sections_report_json;
pub(super) use json::{print_sections_report_json, report_json_with_agents};
pub(super) use table::print_table;
#[cfg(test)]
pub(super) use table::{all_report_title, all_table_columns, all_table_row, source_table_row};

fn totals(rows: &[ReportRow]) -> Totals {
    Totals::from_rows(rows.iter().map(|row| TotalsRow {
        input_tokens: row.input_tokens,
        output_tokens: row.output_tokens,
        cache_creation_tokens: row.cache_creation_tokens,
        cache_read_tokens: row.cache_read_tokens,
        total_tokens: row.total_tokens,
        cost: row.total_cost,
        credits: None,
        model_breakdowns: &row.model_breakdowns,
    }))
}

pub(super) fn sort_rows(rows: &mut [ReportRow], order: &SortOrder) {
    ccusage_core::sort::stable_by(rows, |a, b| {
        a.period
            .cmp(&b.period)
            .then_with(|| a.agent().cmp(b.agent()))
    });
    if *order == SortOrder::Desc {
        rows.reverse();
    }
}
