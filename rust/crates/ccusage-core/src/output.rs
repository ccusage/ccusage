mod format;
mod json;
mod table;
mod warnings;

pub use format::{format_currency, format_models_multiline, format_number, sanitize_terminal_text};
pub use json::{
    agent_summary_json, attach_estimated_models, attach_unpriced_models, json_float,
    json_value_u64, non_empty_json_string, print_json_or_jq, strip_cost_json, summary_period,
    totals_json, wants_json, write_json_or_jq,
};
pub(crate) use json::{estimated_models, unpriced_models};
pub use table::{
    UsageTableOptions, first_column, print_usage_table, print_usage_table_with_options,
    should_use_compact_layout,
};
pub use warnings::{
    print_estimated_pricing_notes_for_models, print_missing_pricing_warnings_for_models,
};

#[cfg(test)]
mod tests;
