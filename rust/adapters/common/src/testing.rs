use ccusage_core::{
    Diagnostics, LoadedEntry, PricingMap, ReportWindow, Result,
    cli::{AgentReportKind, SharedArgs},
};

use crate::source::{EntrySource, LoadContext, ReportPricing, SourceOptions};

pub fn load_entries(
    source: &dyn EntrySource,
    shared: &SharedArgs,
    options: SourceOptions<'_>,
    pricing: Option<&PricingMap>,
) -> Result<(Vec<LoadedEntry>, Diagnostics)> {
    let window = ReportWindow::from_shared(shared);
    let diagnostics = Diagnostics::new(source.descriptor().id);
    let pricing = ReportPricing::for_load(shared, pricing);
    let entries = source.load_entries(&LoadContext {
        shared,
        kind: AgentReportKind::Daily,
        window: &window,
        pricing,
        options,
        diagnostics: &diagnostics,
    })?;
    Ok((entries, diagnostics))
}

pub fn entries(
    source: &dyn EntrySource,
    shared: &SharedArgs,
    options: SourceOptions<'_>,
    pricing: Option<&PricingMap>,
) -> Result<Vec<LoadedEntry>> {
    load_entries(source, shared, options, pricing).map(|(entries, _)| entries)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use ccusage_core::PricingMap;

    use super::load_entries;
    use crate::{
        source::{EntryDescriptor, SourceOptions},
        test_source::{Fake, entries, shared},
    };

    #[test]
    fn hands_back_the_entries_as_loaded_with_the_pipeline_pricing() {
        let source = Fake::new(EntryDescriptor::standard("fake", "Fake"), entries());
        let pricing = PricingMap::default();

        let (loaded, diagnostics) = load_entries(
            &source,
            &shared(Some("20260102"), None),
            SourceOptions::default(),
            Some(&pricing),
        )
        .unwrap();

        assert_eq!(loaded.len(), 4, "the window is the caller's to apply");
        assert!(source.saw_pricing.load(Ordering::Relaxed));
        assert_eq!(diagnostics.source(), "fake");
    }
}
