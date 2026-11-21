use ccusage_adapter_common::{dedupe::dedupe_first_wins, source::LoadContext};

use crate::{LoadedEntry, Result};

use super::parser;

pub(crate) fn load_entries_with_context(cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
    let shared = cx.shared;
    let pricing = cx.pricing.table(cx.shared.mode);
    let files = super::paths::discover_chat_files(cx.diagnostics)?;
    let loaded = ccusage_adapter_common::read_units_flat(
        &files,
        shared.single_thread,
        cx.diagnostics,
        |file| parser::read_chat_file(file, cx.window.timezone(), pricing, cx.diagnostics),
    );
    let mut entries = dedupe_first_wins(loaded, parser::entry_id)
        .into_iter()
        .map(|entry| entry.entry)
        .collect::<Vec<_>>();
    LoadedEntry::sort_by_timestamp(&mut entries);
    Ok(entries)
}
