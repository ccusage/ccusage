use ccusage_adapter_common::{dedupe::dedupe_first_wins, source::LoadContext};

use crate::{LoadedEntry, PricingMap, Result};

use super::parser;

pub(crate) fn load_entries_with_context(cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
    let shared = cx.shared;
    let owned_pricing = if shared.mode == crate::cli::CostMode::Display || cx.pricing.is_some() {
        None
    } else {
        Some(PricingMap::load_with_overrides(
            shared.offline,
            crate::log_level() != Some(0),
            shared.pricing_overrides.iter(),
        ))
    };
    let pricing = cx.pricing.or(owned_pricing.as_ref());
    let files = super::paths::discover_chat_files(cx.diagnostics)?;
    let loaded = ccusage_adapter_common::read_units_flat(
        &files,
        shared.single_thread,
        cx.diagnostics,
        |file| {
            parser::read_chat_file(
                file,
                cx.window.timezone(),
                shared.mode,
                pricing,
                cx.diagnostics,
            )
        },
    );
    let mut entries = dedupe_first_wins(loaded, parser::entry_id);
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}
