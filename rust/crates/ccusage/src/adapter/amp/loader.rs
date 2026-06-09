use crate::{
    LoadedEntry, PricingMap, Result, cli::SharedArgs, collect_files_with_extension, debug_log,
    parse_tz,
};

use super::{parser, paths};

pub(crate) fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(crate::progress::UsageLoadAgent::Amp, shared.json, || {
        load_entries_inner(shared, pricing)
    })
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let mut files = Vec::new();
    for path in paths::paths()? {
        let threads_dir = path.join("threads");
        collect_files_with_extension(&threads_dir, "json", &mut files);
    }
    let mut entries = crate::cache::load_with_cache(
        "amp",
        &files,
        shared.single_thread,
        crate::cache::cost_fingerprint(Some(pricing.fingerprint()), shared.mode),
        shared.live_only,
        |file| {
            Ok(
                parser::read_thread_file(file, tz.as_ref(), shared.mode, Some(pricing))
                    .unwrap_or_else(|error| {
                        debug_log(
                            shared,
                            format!("Failed to read Amp thread file {}: {error}", file.display()),
                        );
                        Vec::new()
                    }),
            )
        },
    )?;
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}
