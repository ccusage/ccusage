use ccusage_adapter_common::{collect_files_with_extension, read_units_flat, source::LoadContext};

use crate::{LoadedEntry, Result};

use super::{parser, paths};

pub(crate) fn load_entries(cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
    let mut entries = Vec::new();
    for path in paths::paths()? {
        let threads_dir = path.join("threads");
        let mut files = Vec::new();
        collect_files_with_extension(&threads_dir, "json", &mut files, cx.diagnostics);
        entries.extend(read_units_flat(
            &files,
            cx.shared.single_thread,
            cx.diagnostics,
            |file| {
                parser::read_thread(
                    file,
                    Some(cx.window.timezone()),
                    cx.shared.mode,
                    cx.pricing,
                    cx.diagnostics,
                )
            },
        ));
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}
