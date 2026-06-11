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
    let mode = shared.mode;
    let mut entries = crate::cache::load_with_cache(
        "amp",
        &files,
        shared.single_thread,
        shared.live_only,
        crate::cache::Freshness::FileStat,
        |file| {
            Ok(
                parser::read_thread_file(file, tz.as_ref(), mode, Some(pricing)).unwrap_or_else(
                    |error| {
                        debug_log(
                            shared,
                            format!("Failed to read Amp thread file {}: {error}", file.display()),
                        );
                        Vec::new()
                    },
                ),
            )
        },
        |e| {
            // Amp bills reasoning tokens as output; extra_total_tokens holds them
            // and the logged cost is never used (parse passes cost_usd = None).
            let cost_usage = crate::TokenUsageRaw {
                output_tokens: e
                    .data
                    .message
                    .usage
                    .output_tokens
                    .saturating_add(e.extra_total_tokens),
                cache_creation: None,
                ..e.data.message.usage
            };
            let model = e.data.message.model.as_deref();
            e.cost = crate::calculate_cost_for_usage(model, cost_usage, None, mode, Some(pricing));
            e.missing_pricing_model = crate::missing_pricing_model_for_usage(
                model,
                cost_usage,
                None,
                mode,
                Some(pricing),
            );
        },
    )?;
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}
