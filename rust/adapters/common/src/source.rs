use std::sync::OnceLock;

use serde_json::Value;

use ccusage_core::{
    Diagnostics, LoadedEntry, PricingMap, ReportWindow, Result, UsageSummary,
    cli::{AgentCommandArgs, AgentReportKind, CodexSpeed, SharedArgs, SourceArgs},
    summary_period,
};

use crate::{pipeline, rows::SourceRow};

pub trait UsageSource: Sync {
    fn id(&self) -> &'static str;

    fn label(&self) -> &'static str;

    fn uses_shared_pricing(&self) -> bool {
        true
    }

    fn load_rows(&self, request: &LoadRequest<'_>) -> Result<LoadedRows>;

    fn has_data(&self) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProgressOwner {
    Source,
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
    pub fn pricing_or_load(&self) -> ReportPricing<'_> {
        ReportPricing::for_load(self.shared, self.pricing)
    }
}

pub enum ReportPricing<'a> {
    Display,
    Supplied(&'a PricingMap),
    Loaded(Box<PricingMap>),
}

impl<'a> ReportPricing<'a> {
    pub fn for_load(shared: &SharedArgs, supplied: Option<&'a PricingMap>) -> Self {
        if shared.mode == ccusage_core::cli::CostMode::Display {
            return Self::Display;
        }
        match supplied {
            Some(pricing) => Self::Supplied(pricing),
            None => Self::Loaded(Box::new(PricingMap::load_with_overrides(
                shared.offline,
                ccusage_core::log_level() != Some(0),
                shared.pricing_overrides.iter(),
            ))),
        }
    }

    pub fn get(&self) -> Option<&PricingMap> {
        match self {
            Self::Display => None,
            Self::Supplied(pricing) => Some(pricing),
            Self::Loaded(pricing) => Some(pricing),
        }
    }
    pub fn table(&self, mode: ccusage_core::cli::CostMode) -> ccusage_core::cost::PriceTable<'_> {
        match self {
            Self::Display => ccusage_core::cost::PriceTable::Display,
            Self::Supplied(pricing) => ccusage_core::cost::PriceTable::for_map(mode, pricing),
            Self::Loaded(pricing) => ccusage_core::cost::PriceTable::for_map(mode, pricing),
        }
    }
}

pub struct UsageLoadContext<'a> {
    shared: &'a SharedArgs,
    window: OnceLock<ReportWindow>,
    pricing: OnceLock<PricingMap>,
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

    pub fn table(&self) -> ccusage_core::cost::PriceTable<'_> {
        self.table_for_mode(self.shared.mode)
    }

    pub fn table_for_mode(
        &self,
        mode: ccusage_core::cli::CostMode,
    ) -> ccusage_core::cost::PriceTable<'_> {
        if mode == ccusage_core::cli::CostMode::Display {
            return ccusage_core::cost::PriceTable::Display;
        }
        let map = self.pricing.get_or_init(|| {
            PricingMap::load_with_overrides(
                self.shared.offline,
                ccusage_core::log_level() != Some(0),
                self.shared.pricing_overrides.iter(),
            )
        });
        ccusage_core::cost::PriceTable::for_map(mode, map)
    }
}

pub struct LoadedRows {
    pub rows: Vec<SourceRow>,
    pub detected: bool,
    pub diagnostics: Diagnostics,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SourceOptions<'a> {
    pub claude_project: Option<&'a str>,
    pub pi_path: Option<&'a str>,
    pub pi_agent_dir: Option<&'a str>,
    pub open_claw_path: Option<&'a str>,
    pub codex_speed: CodexSpeed,
}

impl<'a> SourceOptions<'a> {
    pub(crate) fn from_command(args: &'a AgentCommandArgs) -> Self {
        Self {
            claude_project: args.options.claude_project.as_deref(),
            pi_path: args.options.pi_path.as_deref(),
            pi_agent_dir: args.options.pi_agent_dir.as_deref(),
            open_claw_path: args.options.open_claw_path.as_deref(),
            codex_speed: args.options.codex_speed,
        }
    }

    pub fn from_source(source: Option<&'a SourceArgs>) -> Self {
        source.map_or_else(Self::default, |source| Self {
            claude_project: source.options.claude_project.as_deref(),
            pi_path: source.options.pi_path.as_deref(),
            pi_agent_dir: source.options.pi_agent_dir.as_deref(),
            open_claw_path: source.options.open_claw_path.as_deref(),
            codex_speed: source.options.codex_speed,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowMode {
    Entries,
    SessionLastActivity,
}

#[derive(Clone, Copy, Debug)]
pub enum Detection {
    NonEmpty,
    NonEmptyOrHasData(fn() -> bool),
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

    pub(crate) fn for_load(kind: LoadKind) -> Self {
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

#[derive(Clone, Copy, Debug)]
pub struct EntryDescriptor {
    pub id: &'static str,
    pub label: &'static str,
    pub window: WindowMode,
    pub detection: Detection,
    pub unified_project_path: bool,
    pub session: SessionShape,
    pub table: TableStyle,
    pub session_scope: SessionScope,
}

impl EntryDescriptor {
    pub const fn standard(id: &'static str, label: &'static str) -> Self {
        Self {
            id,
            label,
            window: WindowMode::Entries,
            detection: Detection::NonEmpty,
            unified_project_path: false,
            session: SessionShape::Plain,
            table: TableStyle::Standard,
            session_scope: SessionScope::Source,
        }
    }
}

pub struct LoadContext<'a> {
    pub shared: &'a SharedArgs,
    pub kind: AgentReportKind,
    pub window: &'a ReportWindow,
    pub pricing: ReportPricing<'a>,
    pub options: SourceOptions<'a>,
    pub diagnostics: &'a Diagnostics,
}

pub trait EntrySource: Sync {
    fn descriptor(&self) -> EntryDescriptor;

    fn run_focused(&self, args: AgentCommandArgs) -> Result<()>
    where
        Self: Sized,
    {
        pipeline::run_focused(self, args)
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>>;

    fn summarize(&self, entries: &[LoadedEntry], kind: SummaryKind) -> Result<Vec<UsageSummary>> {
        let descriptor = self.descriptor();
        crate::rows::summarize_entries(entries, kind, descriptor.session, descriptor.session_scope)
    }

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
        let pricing = ReportPricing::for_load(&shared, Some(&supplied));

        assert!(std::ptr::eq(pricing.get().unwrap(), &supplied));
    }

    #[test]
    fn display_does_not_consult_even_supplied_pricing() {
        ccusage_test_support::install_pricing_snapshots();
        let shared = SharedArgs {
            mode: CostMode::Display,
            ..Default::default()
        };
        let mut supplied = PricingMap::default();
        supplied.load_json(r#"{"gpt-5":{"input_cost_per_token":1.0,"output_cost_per_token":1.0}}"#);
        assert!(supplied.find("gpt-5").is_some());
        let pricing = ReportPricing::for_load(&shared, Some(&supplied));

        assert!(matches!(pricing, ReportPricing::Display));
        assert!(pricing.get().is_none());
    }

    #[test]
    fn display_does_not_load_pricing() {
        let shared = SharedArgs {
            mode: CostMode::Display,
            ..Default::default()
        };

        assert!(matches!(
            ReportPricing::for_load(&shared, None),
            ReportPricing::Display
        ));
    }

    #[test]
    fn usage_context_resolves_nothing_until_a_file_needs_it() {
        let shared = SharedArgs {
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
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

        assert!(matches!(
            cx.table(),
            ccusage_core::cost::PriceTable::Display
        ));
        assert!(cx.pricing.get().is_none());
        assert!(cx.window.get().is_none());
    }
}
