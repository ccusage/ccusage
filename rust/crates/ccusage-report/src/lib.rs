//! Report-only helpers for the CLI and the Claude report commands.

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
                order: Some(order),
                ..SharedArgs::default()
            };
            assert_eq!(session_cost_ordering(&shared), order);
        }
    }
}
