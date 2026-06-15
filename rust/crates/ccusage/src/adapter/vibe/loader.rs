use crate::{LoadedEntry, Result, cli::SharedArgs};

use super::parser;

pub(crate) fn load_entries(shared: &SharedArgs) -> Result<Vec<LoadedEntry>> {
    let pricing = if shared.mode == crate::cli::CostMode::Display {
        None
    } else {
        Some(crate::PricingMap::load_with_overrides(
            shared.offline,
            crate::log_level() != Some(0),
            shared.pricing_overrides.iter(),
        ))
    };
    parser::load_entries(shared, pricing.as_ref())
}
