use ccusage_adapter_common::{
    collect_files_with_extension, source::LoadContext, units::read_typed_units_flat,
};

use crate::{LoadedEntry, Result};

use super::{parser, paths};

pub(crate) fn load_entries(cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
    let pricing = cx.pricing.table(cx.shared.mode);
    let mut entries = Vec::new();
    for path in paths::paths()? {
        let threads_dir = path.join("threads");
        let mut files = Vec::new();
        collect_files_with_extension(&threads_dir, "json", &mut files, cx.diagnostics);
        entries.extend(read_typed_units_flat(
            &files,
            cx.shared.single_thread,
            cx.diagnostics,
            |file| parser::read_thread(file, cx.window.timezone(), pricing, cx.diagnostics),
        ));
    }
    LoadedEntry::sort_by_timestamp(&mut entries);
    Ok(entries)
}
