//! The contracts between a usage source and the reports that read it.
//!
//! [`UsageSource`] supplies identity, supported reports and unified rows to
//! the unified registry. Sources whose loader yields `Vec<LoadedEntry>` also
//! implement [`EntrySource`], which runs focused and unified reports through
//! [`crate::pipeline`]. Format-specific sources implement [`UsageSource`]
//! directly and keep their focused report paths in their own adapter.

use std::sync::OnceLock;

use serde_json::Value;

use ccusage_core::{
    Diagnostics, LoadedEntry, PricingMap, ReportWindow, Result, UsageSummary,
    cli::{AgentCommandArgs, AgentReportKind, CodexSpeed, SharedArgs, SourceArgs},
    cli_error,
    pricing::PendingPricing,
    summary_period,
};

use crate::{pipeline, rows::UnifiedRow};

/// One usage source registered for the unified report.
pub trait UsageSource: Sync {
    /// The agent name on the command line and in unified rows, such as `amp`.
    fn id(&self) -> &'static str;

    /// The name progress output shows while the source loads, such as `Amp`.
    fn label(&self) -> &'static str;

    fn pricing_supply(&self) -> PricingSupply {
        PricingSupply::Pipeline
    }

    fn load_rows(&self, request: &LoadRequest<'_>) -> Result<LoadedRows>;

    /// Whether the source has usage data at all, whatever the date window.
    /// Only sources that cannot tell from their loaded rows answer.
    fn has_data(&self) -> bool {
        false
    }
}

/// Who renders the progress of a load.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProgressOwner {
    /// The source shows its own spinner, as a focused report does.
    Source,
    /// The caller already tracks every source it loads, as the unified report
    /// does, so the source stays quiet.
    Caller,
}

impl ProgressOwner {
    pub fn run<T, E>(
        self,
        agent: ccusage_core::progress::UsageLoadAgent,
        json: bool,
        load: impl FnOnce() -> std::result::Result<T, E>,
    ) -> std::result::Result<T, E> {
        match self {
            Self::Source => ccusage_core::progress::track_usage_load(agent, json, load),
            Self::Caller => load(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadKind {
    Day,
    Session,
}

impl LoadKind {
    pub fn for_report(kind: AgentReportKind) -> Self {
        match kind {
            AgentReportKind::Session => Self::Session,
            AgentReportKind::Daily | AgentReportKind::Weekly | AgentReportKind::Monthly => {
                Self::Day
            }
        }
    }

    pub fn report_kind(self) -> AgentReportKind {
        match self {
            Self::Day => AgentReportKind::Daily,
            Self::Session => AgentReportKind::Session,
        }
    }
}

pub struct LoadRequest<'a> {
    pub kind: LoadKind,
    pub shared: &'a SharedArgs,
    pub options: SourceOptions<'a>,
    pub pricing: Option<&'a PricingMap>,
    pub progress: ProgressOwner,
}

impl LoadRequest<'_> {
    pub fn pricing_or_load(&self) -> Result<ReportPricing<'_>> {
        ReportPricing::for_load(self.shared, self.pricing)
    }
}

pub enum ReportPricing<'a> {
    Display,
    Supplied(&'a PricingMap),
    Pending(PendingPricing),
}

impl<'a> ReportPricing<'a> {
    pub fn for_load(shared: &SharedArgs, supplied: Option<&'a PricingMap>) -> Result<Self> {
        if shared.mode == ccusage_core::cli::CostMode::Display {
            return Ok(Self::Display);
        }
        match supplied {
            Some(pricing) => Ok(Self::Supplied(pricing)),
            None => PricingMap::spawn_load(
                shared.offline,
                ccusage_core::log_level() != Some(0),
                shared.pricing_overrides.iter(),
            )
            .map(Self::Pending),
        }
    }

    pub fn get(&self) -> Result<&PricingMap> {
        static DISPLAY: OnceLock<PricingMap> = OnceLock::new();
        match self {
            Self::Display => Ok(DISPLAY.get_or_init(PricingMap::default)),
            Self::Supplied(pricing) => Ok(pricing),
            Self::Pending(pricing) => pricing.join(),
        }
    }
}

pub enum LoaderPricing<'a> {
    Pipeline(ReportPricing<'a>),
    Source,
}

pub struct UsageLoadContext<'a> {
    pub shared: &'a SharedArgs,
    window: OnceLock<ReportWindow>,
    pricing: OnceLock<Option<PricingMap>>,
}

impl<'a> UsageLoadContext<'a> {
    pub fn new(shared: &'a SharedArgs) -> Self {
        Self {
            shared,
            window: OnceLock::new(),
            pricing: OnceLock::new(),
        }
    }

    pub fn window(&self) -> &ReportWindow {
        self.window
            .get_or_init(|| ReportWindow::from_shared(self.shared))
    }

    pub fn pricing(&self) -> Option<&PricingMap> {
        self.pricing
            .get_or_init(|| {
                (self.shared.mode != ccusage_core::cli::CostMode::Display).then(|| {
                    PricingMap::load_with_overrides(
                        self.shared.offline,
                        ccusage_core::log_level() != Some(0),
                        self.shared.pricing_overrides.iter(),
                    )
                })
            })
            .as_ref()
    }
}

pub struct LoadedRows {
    pub rows: Vec<UnifiedRow>,
    pub detected: bool,
    pub diagnostics: Diagnostics,
}

/// The per-source options beside [`SharedArgs`]. Each is read by one source.
#[derive(Clone, Copy, Debug, Default)]
pub struct SourceOptions<'a> {
    /// Claude: keep only this project.
    pub claude_project: Option<&'a str>,
    /// pi: the sessions directory to read instead of the default one.
    pub pi_path: Option<&'a str>,
    /// OpenClaw: the directory to read instead of the default one.
    pub open_claw_path: Option<&'a str>,
    /// Codex: the service tier to price with.
    pub codex_speed: CodexSpeed,
}

impl<'a> SourceOptions<'a> {
    /// The options of `ccusage <agent> <kind>`.
    pub fn from_command(args: &'a AgentCommandArgs) -> Self {
        Self {
            claude_project: args.options.claude_project.as_deref(),
            pi_path: args.options.pi_path.as_deref(),
            open_claw_path: args.options.open_claw_path.as_deref(),
            codex_speed: args.options.codex_speed,
        }
    }

    /// The options the configuration resolved for one source of a unified
    /// report, or the defaults for a source it does not address.
    pub fn from_source(source: Option<&'a SourceArgs>) -> Self {
        source.map_or_else(Self::default, |source| Self {
            claude_project: source.options.claude_project.as_deref(),
            pi_path: source.options.pi_path.as_deref(),
            open_claw_path: source.options.open_claw_path.as_deref(),
            codex_speed: source.options.codex_speed,
        })
    }
}

/// Where a source's date window is applied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowMode {
    /// Entries outside the window are dropped before summarizing. A session
    /// that crosses the window edge is cut at it.
    Entries,
    /// As [`Entries`](Self::Entries), except that a session report summarizes
    /// every entry and keeps the sessions whose last activity is inside the
    /// window, so a kept session shows its whole usage.
    SessionLastActivity,
}

/// How the unified report decides that a source is present.
#[derive(Clone, Copy, Debug)]
pub enum Detection {
    /// The source loaded at least one entry, before the date window.
    NonEmpty,
    /// As [`NonEmpty`](Self::NonEmpty), or the probe finds data on disk; for a
    /// source whose loader can return nothing while data exists.
    NonEmptyOrHasData(fn() -> bool),
}

/// Who supplies the pricing a source's loader needs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PricingSupply {
    /// The pipeline hands the loader a [`PricingMap`]: the unified report's
    /// shared one, or one it loads itself.
    Pipeline,
    /// The loader loads pricing itself, when and if it needs it.
    Source,
}

#[derive(Clone, Copy, Debug)]
pub enum SummaryKind {
    Daily,
    Monthly,
    Weekly(ccusage_core::cli::WeekDay),
    Session,
}

impl SummaryKind {
    pub fn for_report(kind: AgentReportKind, start: ccusage_core::cli::WeekDay) -> Self {
        match kind {
            AgentReportKind::Daily => Self::Daily,
            AgentReportKind::Monthly => Self::Monthly,
            AgentReportKind::Weekly => Self::Weekly(start),
            AgentReportKind::Session => Self::Session,
        }
    }

    pub fn for_load(kind: LoadKind) -> Self {
        match kind {
            LoadKind::Day => Self::Daily,
            LoadKind::Session => Self::Session,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionScope {
    Source,
    Project,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionShape {
    Plain,
    WithActivity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableStyle {
    Standard,
    Credits,
    AutoCacheCreation,
}

/// The identity and the declared policies of an [`EntrySource`].
#[derive(Clone, Copy, Debug)]
pub struct EntryDescriptor {
    pub id: &'static str,
    pub label: &'static str,
    pub window: WindowMode,
    pub detection: Detection,
    pub pricing: PricingSupply,
    /// Whether unified session rows carry the session's project path.
    pub unified_project_path: bool,
    pub session: SessionShape,
    pub table: TableStyle,
    pub session_scope: SessionScope,
}

impl EntryDescriptor {
    /// A source with the common policies: daily, monthly and session reports,
    /// the window applied to entries, detected by loading at least one entry,
    /// priced by the pipeline's map.
    pub const fn standard(id: &'static str, label: &'static str) -> Self {
        Self {
            id,
            label,
            window: WindowMode::Entries,
            detection: Detection::NonEmpty,
            pricing: PricingSupply::Pipeline,
            unified_project_path: false,
            session: SessionShape::Plain,
            table: TableStyle::Standard,
            session_scope: SessionScope::Source,
        }
    }
}

/// What a loader is given for one load, focused or unified.
pub struct LoadContext<'a, P = Option<&'a PricingMap>> {
    pub shared: &'a SharedArgs,
    /// The kind being loaded, for a loader whose input depends on it.
    pub kind: AgentReportKind,
    /// The report window, as a hint for a loader that can skip input early.
    pub window: &'a ReportWindow,
    /// Present for [`PricingSupply::Pipeline`] sources.
    pub pricing: P,
    pub options: SourceOptions<'a>,
    /// Where the loader records what it skipped.
    pub diagnostics: &'a Diagnostics,
}

impl<'a> LoadContext<'a> {
    /// The pricing a [`PricingSupply::Pipeline`] source is handed.
    ///
    /// # Errors
    ///
    /// Returns an error for a context built without it, which the pipeline
    /// never does for such a source.
    pub fn pipeline_pricing(&self) -> Result<&'a PricingMap> {
        self.pricing
            .ok_or_else(|| cli_error("the report pipeline did not supply pricing"))
    }
}

/// A source whose loader yields `Vec<LoadedEntry>`.
pub trait EntrySource: Sync {
    fn descriptor(&self) -> EntryDescriptor;

    fn load_priced_entries(
        &self,
        cx: &LoadContext<'_, LoaderPricing<'_>>,
    ) -> Result<Vec<LoadedEntry>> {
        self.load_entries(&LoadContext {
            shared: cx.shared,
            kind: cx.kind,
            window: cx.window,
            pricing: match &cx.pricing {
                LoaderPricing::Pipeline(pricing) => Some(pricing.get()?),
                LoaderPricing::Source => None,
            },
            options: cx.options,
            diagnostics: cx.diagnostics,
        })
    }

    /// Runs `ccusage <id> <kind>` through the shared entry pipeline.
    fn run_focused(&self, args: AgentCommandArgs) -> Result<()>
    where
        Self: Sized,
    {
        pipeline::run_focused(self, args)
    }

    /// Loads every entry, sorted the way the adapter's reports expect. The
    /// pipeline shows progress and applies the date window, so the loader does
    /// neither. A loader that narrows to [`LoadContext::window`] as it reads
    /// can return nothing while the source has data, which is what
    /// [`Detection::NonEmptyOrHasData`] is for.
    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>>;

    fn summarize(&self, entries: &[LoadedEntry], kind: SummaryKind) -> Result<Vec<UsageSummary>> {
        let descriptor = self.descriptor();
        crate::rows::summarize_entries(entries, kind, descriptor.session, descriptor.session_scope)
    }

    /// The key `--order` sorts focused rows by.
    fn sort_key<'a>(&self, row: &'a UsageSummary) -> &'a str {
        summary_period(row)
    }

    fn report_json(&self, rows: &[UsageSummary], kind: AgentReportKind) -> Value {
        crate::report::report_json(
            rows,
            kind,
            self.descriptor().session == SessionShape::WithActivity,
        )
    }

    fn print_table(
        &self,
        rows: &[UsageSummary],
        kind: AgentReportKind,
        shared: &SharedArgs,
    ) -> Result<()> {
        crate::report::print_table(self.descriptor(), rows, kind, shared)
    }
}

impl<S: EntrySource> UsageSource for S {
    fn id(&self) -> &'static str {
        self.descriptor().id
    }

    fn label(&self) -> &'static str {
        self.descriptor().label
    }

    fn pricing_supply(&self) -> PricingSupply {
        self.descriptor().pricing
    }

    fn load_rows(&self, request: &LoadRequest<'_>) -> Result<LoadedRows> {
        pipeline::load_rows(self, request)
    }

    fn has_data(&self) -> bool {
        match self.descriptor().detection {
            Detection::NonEmpty => false,
            Detection::NonEmptyOrHasData(has_data) => has_data(),
        }
    }
}

#[cfg(test)]
mod pricing_tests {
    use super::{ReportPricing, UsageLoadContext};
    use ccusage_core::{
        PricingMap,
        cli::{CostMode, SharedArgs},
    };

    #[test]
    fn supplied_pricing_is_borrowed_without_loading_a_base() {
        let shared = SharedArgs::default();
        let supplied = PricingMap::default();
        let pricing = ReportPricing::for_load(&shared, Some(&supplied)).unwrap();

        assert!(std::ptr::eq(pricing.get().unwrap(), &supplied));
    }

    #[test]
    fn display_does_not_consult_even_supplied_pricing() {
        let shared = SharedArgs {
            mode: CostMode::Display,
            ..Default::default()
        };
        let mut supplied = PricingMap::default();
        supplied.load_json(r#"{"gpt-5":{"input_cost_per_token":1.0,"output_cost_per_token":1.0}}"#);
        assert!(supplied.find("gpt-5").is_some());
        let pricing = ReportPricing::for_load(&shared, Some(&supplied)).unwrap();

        assert!(matches!(pricing, ReportPricing::Display));
        assert!(pricing.get().unwrap().find("gpt-5").is_none());
    }

    #[test]
    fn display_does_not_start_a_pending_load() {
        let shared = SharedArgs {
            mode: CostMode::Display,
            ..Default::default()
        };

        assert!(matches!(
            ReportPricing::for_load(&shared, None).unwrap(),
            ReportPricing::Display
        ));
    }

    #[test]
    fn usage_context_resolves_nothing_until_a_file_needs_it() {
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..Default::default()
        };
        let cx = UsageLoadContext::new(&shared);

        assert!(cx.window.get().is_none());
        assert!(cx.pricing.get().is_none());
        assert!(std::ptr::eq(cx.window(), cx.window()));
        assert!(cx.pricing.get().is_none());
    }

    #[test]
    fn display_usage_context_never_loads_pricing() {
        let shared = SharedArgs {
            mode: CostMode::Display,
            ..Default::default()
        };
        let cx = UsageLoadContext::new(&shared);

        assert!(cx.pricing().is_none());
        assert!(cx.pricing.get().is_some_and(Option::is_none));
        assert!(cx.window.get().is_none());
    }
}
