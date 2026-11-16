//! Test support: loads a source's entries the way its reports do and keeps
//! what the load skipped, so a test can assert on both.

use ccusage_core::{
    Diagnostics, LoadedEntry, PricingMap, ReportWindow, Result,
    cli::{AgentReportKind, SharedArgs},
};

use crate::source::{EntrySource, LoadContext, SourceOptions};

/// Loads `source` with `shared` and `options` as a daily report would, before
/// the date window is applied. `pricing` is what the pipeline would hand a
/// source that asks it for pricing.
///
/// # Errors
///
/// Returns the error the load returns.
pub fn load_entries(
    source: &dyn EntrySource,
    shared: &SharedArgs,
    options: SourceOptions<'_>,
    pricing: Option<&PricingMap>,
) -> Result<(Vec<LoadedEntry>, Diagnostics)> {
    let window = ReportWindow::from_shared(shared);
    let diagnostics = Diagnostics::new(source.descriptor().id);
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
