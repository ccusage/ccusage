//! Report-only helpers for the CLI and the Claude report commands.
//!
//! They live here rather than in `ccusage-core` so editing them recompiles
//! only the Claude adapter and the binary, not every adapter built on core.

mod date_utils;
mod last_window;
mod output;
mod summary;

pub use date_utils::{
    am_pm, format_date, format_utc_second, hour_12, is_valid_timezone, local_parts,
};
pub use last_window::last_periods_since;
pub use output::{group_project_output, session_summary_json, summary_json};
pub use summary::filter_and_sort_summaries;

pub fn session_cost_ordering(
    shared: &ccusage_core::cli::SharedArgs,
) -> ccusage_core::cli::SortOrder {
    shared
        .requested_order()
        .unwrap_or(ccusage_core::cli::SortOrder::Desc)
}

#[cfg(test)]
mod ordering_tests {
    use super::*;
    use ccusage_core::cli::{SharedArgs, SortOrder};

    #[test]
    fn session_cost_order_defaults_to_descending_and_preserves_explicit_orders() {
        assert_eq!(
            session_cost_ordering(&SharedArgs::default()),
            SortOrder::Desc
        );
        for order in [SortOrder::Asc, SortOrder::Desc] {
            let shared = SharedArgs {
                order,
                order_explicit: true,
                ..SharedArgs::default()
            };
            assert_eq!(session_cost_ordering(&shared), order);
        }
    }
}
