//! The contracts between a usage source and the reports that read it.
//!
//! [`UsageSource`] supplies identity, supported reports and unified rows to
//! the unified registry. Sources whose loader yields `Vec<LoadedEntry>` also
//! implement [`EntrySource`], which runs focused and unified reports through
//! [`crate::pipeline`]. Format-specific sources implement [`UsageSource`]
//! directly and keep their focused report paths in their own adapter.

use serde_json::Value;

use ccusage_core::{
    Diagnostics, LoadedEntry, PricingMap, ReportWindow, Result, UsageSummary,
    cli::{AgentCommandArgs, AgentId, AgentReportKind, CodexSpeed, SharedArgs, SourceArgs},
    cli_error, summary_period,
};

use crate::{claims::ClaudeCounted, pipeline, rows::UnifiedRow};

/// One usage source registered for the unified report.
pub trait UsageSource: Sync {
    /// The agent name on the command line and in unified rows, such as `amp`.
    fn id(&self) -> &'static str;

    /// The name progress output shows while the source loads, such as `Amp`.
    fn label(&self) -> &'static str;

    /// The report kinds `ccusage <id> <kind>` accepts.
    fn reports(&self) -> &'static [AgentReportKind] {
        AgentId::from_name(self.id()).map_or(&[], |agent| agent.spec().reports)
    }

    fn pricing_supply(&self) -> PricingSupply {
        PricingSupply::Pipeline
    }

    /// Loads the rows this source contributes to the unified report.
    ///
    /// `request.kind` is the kind the unified report loads at, not the one it
    /// prints: weekly and monthly reports load `Daily` rows and bucket them
    /// afterwards, across sources, with the report's own week start. A source
    /// must not bucket by week or month here.
    fn unified_rows(&self, request: &UnifiedRequest<'_>) -> Result<SourceRows>;

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
            claude_project: None,
            pi_path: args.pi_path.as_deref(),
            open_claw_path: args.open_claw_path.as_deref(),
            codex_speed: args.codex_speed,
        }
    }

    /// The options the configuration resolved for one source of a unified
    /// report, or the defaults for a source it does not address.
    pub fn from_source(source: Option<&'a SourceArgs>) -> Self {
        source.map_or_else(Self::default, |source| Self {
            claude_project: source.claude_project.as_deref(),
            pi_path: source.pi_path.as_deref(),
            open_claw_path: source.open_claw_path.as_deref(),
            codex_speed: source.codex_speed,
        })
    }
}

/// What the unified report asks one source for.
pub struct UnifiedRequest<'a> {
    /// The kind to load at; see [`UsageSource::unified_rows`].
    pub kind: AgentReportKind,
    /// The options this source loads with: the ones the configuration resolved
    /// for it, or the unified report's own.
    pub shared: &'a SharedArgs,
    pub options: SourceOptions<'a>,
    /// The pricing the unified report shares between sources with the same
    /// pricing options. `None` for a source that loads its own.
    pub pricing: Option<&'a PricingMap>,
    pub progress: ProgressOwner,
    /// Codex only: take the driver that merges every source directory even
    /// when there is a single one, as its JSON output does. The unified report
    /// always sets it.
    pub multi_source_driver: bool,
    /// OpenCode only: blocks until the Claude source of the same report has
    /// loaded and yields what it counted.
    pub claude_counted: Option<&'a (dyn Fn() -> ClaudeCounted + Send)>,
}

/// What one source contributes to the unified report.
pub struct SourceRows {
    pub rows: Vec<UnifiedRow>,
    /// Whether the report lists the source among the agents it found, which
    /// must not depend on the date window.
    pub detected: bool,
    /// What the load skipped. The unified report merges it with other loads
    /// of the same source and prints it once all progress output stopped.
    pub diagnostics: Diagnostics,
    /// Claude only: the usage it counted, for the sources that would otherwise
    /// count the same turns.
    pub claude_counted: Option<ClaudeCounted>,
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
        }
    }
}

/// What a loader is given for one load, focused or unified.
pub struct LoadContext<'a> {
    pub shared: &'a SharedArgs,
    /// The kind being loaded, for a loader whose input depends on it.
    pub kind: AgentReportKind,
    /// The report window, as a hint for a loader that can skip input early.
    pub window: &'a ReportWindow,
    /// Present for [`PricingSupply::Pipeline`] sources.
    pub pricing: Option<&'a PricingMap>,
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
///
/// The hooks are the adapter's own functions: how it summarizes and renders
/// decides float summation order, JSON shape and table layout, so none of that
/// is shared. What is shared is the order they run in, which
/// [`crate::pipeline`] owns.
pub trait EntrySource: Sync {
    fn descriptor(&self) -> EntryDescriptor;

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

    fn summarize(
        &self,
        entries: &[LoadedEntry],
        kind: AgentReportKind,
    ) -> Result<Vec<UsageSummary>>;

    /// Summarizes for the unified report, for a source that groups its unified
    /// rows differently from its focused ones.
    fn summarize_unified(
        &self,
        entries: &[LoadedEntry],
        kind: AgentReportKind,
    ) -> Result<Vec<UsageSummary>> {
        self.summarize(entries, kind)
    }

    /// The key `--order` sorts focused rows by.
    fn sort_key<'a>(&self, row: &'a UsageSummary) -> &'a str {
        summary_period(row)
    }

    fn report_json(&self, rows: &[UsageSummary], kind: AgentReportKind) -> Value;

    fn print_table(
        &self,
        rows: &[UsageSummary],
        kind: AgentReportKind,
        shared: &SharedArgs,
    ) -> Result<()>;
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

    fn unified_rows(&self, request: &UnifiedRequest<'_>) -> Result<SourceRows> {
        pipeline::unified_rows(self, request)
    }

    fn has_data(&self) -> bool {
        match self.descriptor().detection {
            Detection::NonEmpty => false,
            Detection::NonEmptyOrHasData(has_data) => has_data(),
        }
    }
}
