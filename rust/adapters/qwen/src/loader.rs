use ccusage_adapter_common::{
    pipeline,
    source::{LoadContext, SourceOptions},
};

use crate::{LoadedEntry, PricingMap, Result, cli::SharedArgs};

use super::parser;

pub fn load_entries(shared: &SharedArgs) -> Result<Vec<LoadedEntry>> {
    pipeline::load_entries_for_unmigrated_caller(
        &crate::SOURCE,
        shared,
        SourceOptions::default(),
        None,
    )
}

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
    let files = super::paths::discover_chat_files_with_diagnostics(cx.diagnostics)?;
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
                shared,
                cx.diagnostics,
            )
        },
    );
    let mut entries = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for entry in loaded {
        if seen.insert(parser::entry_id(&entry)) {
            entries.push(entry);
        }
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}
