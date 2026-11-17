//! Test support: checks that a source counts the same usage in its focused
//! report and in its slice of the unified report.
//!
//! The two reports share the pipeline, but each adapter still supplies the
//! hooks, the policies and a separate unified summarizer, so they can drift.
//! Compiled only for tests: this crate's own, and an adapter's through the
//! `testing` feature it enables from its `[dev-dependencies]`.

use std::collections::BTreeMap;

use serde_json::Value;

use ccusage_core::cli::{
    AgentCommandArgs, AgentId, AgentReportKind, CodexSpeed, SharedArgs, WeekDay,
};

use crate::{
    pipeline,
    source::{EntrySource, ProgressOwner, SourceOptions, UnifiedRequest},
};

/// A report kind whose focused and unified totals are known to differ, so the
/// difference is on record instead of failing the check.
pub struct KnownDivergence {
    pub kind: AgentReportKind,
    /// The exact difference, and why it is not a bug to fix here.
    pub reason: &'static str,
}

#[derive(Debug, PartialEq)]
struct Totals {
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    total_tokens: u64,
    total_cost: f64,
}

impl Totals {
    /// Costs are summed in a different order by the two reports, so they are
    /// compared up to rounding; token counts are compared exactly.
    fn matches(&self, other: &Self) -> bool {
        let scale = self.total_cost.abs().max(other.total_cost.abs()).max(1.0);
        self.input_tokens == other.input_tokens
            && self.output_tokens == other.output_tokens
            && self.cache_creation_tokens == other.cache_creation_tokens
            && self.cache_read_tokens == other.cache_read_tokens
            && self.total_tokens == other.total_tokens
            && (self.total_cost - other.total_cost).abs() <= scale * 1e-9
    }
}

/// Asserts that, for every report kind `source` supports, the `totals` of its
/// focused JSON report equal the totals of the rows it gives the unified
/// report when both load with `shared`.
///
/// The caller points the source at a fixture first, through the environment
/// variable its `paths.rs` reads. The fixture must hold usage: two empty
/// reports agree without comparing anything, so a source that reads nothing
/// without the window fails the check. A window that leaves nothing is still
/// compared. A kind listed in `known` must differ: an entry that no longer
/// does fails the check, so the list cannot go stale.
///
/// # Panics
///
/// Panics when a load fails, when the source reads no usage, when a kind
/// differs without being listed, or when a listed kind no longer differs.
pub fn assert_focused_matches_unified<S: EntrySource>(
    source: &S,
    agent: AgentId,
    shared: &SharedArgs,
    known: &[KnownDivergence],
) {
    let descriptor = source.descriptor();
    let unbounded = SharedArgs {
        since: None,
        until: None,
        ..shared.clone()
    };
    let everything = focused_totals(source, AgentReportKind::Daily, &unbounded);
    assert!(
        everything.total_tokens > 0 || everything.total_cost > 0.0,
        "{}: the focused report read no usage, so there is nothing to compare; point the source at its fixture first",
        descriptor.id
    );
    assert_eq!(
        descriptor.id,
        agent.spec().name,
        "the source is not the agent whose reports it is checked for"
    );
    for kind in agent.spec().reports.iter().copied() {
        let focused = focused_totals(source, kind, shared);
        let unified = unified_totals(source, kind, shared);
        let matches = focused.matches(&unified);
        match known.iter().find(|divergence| divergence.kind == kind) {
            Some(divergence) => assert!(
                !matches,
                "{} {kind:?}: focused and unified totals agree, so drop the known divergence ({})",
                descriptor.id, divergence.reason
            ),
            None => assert!(
                matches,
                "{} {kind:?}: focused totals {focused:?} differ from the unified slice {unified:?} (since {:?}, until {:?}, timezone {:?})",
                descriptor.id, shared.since, shared.until, shared.timezone
            ),
        }
    }
}

fn focused_totals<S: EntrySource>(
    source: &S,
    kind: AgentReportKind,
    shared: &SharedArgs,
) -> Totals {
    let args = AgentCommandArgs {
        shared: shared.clone(),
        kind,
        session_id: None,
        sections: None,
        by_agent: false,
        pi_path: None,
        open_claw_path: None,
        codex_speed: CodexSpeed::default(),
        include_claude_agent: false,
        start_of_week: WeekDay::Sunday,
        sources: BTreeMap::new(),
    };
    let rows = pipeline::focused_rows(source, &args)
        .unwrap_or_else(|error| panic!("focused {kind:?} load failed: {error}"));
    let report = source.report_json(&rows, kind);
    let totals = &report["totals"];
    let tokens = |key: &str| totals.get(key).and_then(Value::as_u64).unwrap_or(0);
    Totals {
        input_tokens: tokens("inputTokens"),
        output_tokens: tokens("outputTokens"),
        cache_creation_tokens: tokens("cacheCreationTokens"),
        cache_read_tokens: tokens("cacheReadTokens"),
        total_tokens: tokens("totalTokens"),
        total_cost: totals
            .get("totalCost")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
    }
}

fn unified_totals<S: EntrySource>(
    source: &S,
    kind: AgentReportKind,
    shared: &SharedArgs,
) -> Totals {
    let load_kind = crate::source::LoadKind::for_report(kind).report_kind();
    let rows = pipeline::unified_rows(
        source,
        &UnifiedRequest {
            kind: load_kind,
            shared,
            options: SourceOptions::default(),
            pricing: None,
            progress: ProgressOwner::Caller,
            claude_counted: None,
        },
    )
    .unwrap_or_else(|error| panic!("unified {kind:?} load failed: {error}"))
    .rows;
    let sum = |tokens: fn(&crate::rows::UnifiedRow) -> u64| {
        rows.iter().map(tokens).fold(0, u64::saturating_add)
    };
    Totals {
        input_tokens: sum(|row| row.input_tokens),
        output_tokens: sum(|row| row.output_tokens),
        cache_creation_tokens: sum(|row| row.cache_creation_tokens),
        cache_read_tokens: sum(|row| row.cache_read_tokens),
        total_tokens: sum(|row| row.total_tokens),
        total_cost: rows.iter().map(|row| row.total_cost).sum(),
    }
}

#[cfg(test)]
mod tests {
    use ccusage_core::cli::{AgentId, AgentReportKind};

    use super::{KnownDivergence, assert_focused_matches_unified};
    use crate::test_source::{Fake, entries, shared};

    fn descriptor() -> crate::source::EntryDescriptor {
        crate::source::EntryDescriptor {
            id: "amp",
            ..crate::test_source::descriptor()
        }
    }

    fn drifted() -> Fake {
        Fake {
            unified_drops_an_entry: true,
            ..Fake::new(descriptor(), entries())
        }
    }

    fn every_kind_diverges() -> Vec<KnownDivergence> {
        [
            AgentReportKind::Daily,
            AgentReportKind::Monthly,
            AgentReportKind::Session,
        ]
        .into_iter()
        .map(|kind| KnownDivergence {
            kind,
            reason: "the stand-in drops an entry from its unified summary",
        })
        .collect()
    }

    #[test]
    fn passes_a_source_whose_reports_agree() {
        let source = Fake::new(descriptor(), entries());

        assert_focused_matches_unified(&source, AgentId::Amp, &shared(None, None), &[]);
        assert_focused_matches_unified(&source, AgentId::Amp, &shared(Some("20260102"), None), &[]);
    }

    #[test]
    fn compares_a_window_that_leaves_nothing() {
        let source = Fake::new(descriptor(), entries());

        assert_focused_matches_unified(&source, AgentId::Amp, &shared(Some("20300101"), None), &[]);
    }

    #[test]
    #[should_panic(expected = "the focused report read no usage")]
    fn fails_a_source_that_reads_no_usage() {
        let source = Fake::new(descriptor(), Vec::new());

        assert_focused_matches_unified(&source, AgentId::Amp, &shared(None, None), &[]);
    }

    #[test]
    #[should_panic(expected = "differ from the unified slice")]
    fn fails_a_source_whose_unified_slice_drifted() {
        assert_focused_matches_unified(&drifted(), AgentId::Amp, &shared(None, None), &[]);
    }

    #[test]
    fn accepts_a_divergence_that_is_on_record() {
        assert_focused_matches_unified(
            &drifted(),
            AgentId::Amp,
            &shared(None, None),
            &every_kind_diverges(),
        );
    }

    #[test]
    #[should_panic(expected = "drop the known divergence")]
    fn fails_a_recorded_divergence_that_no_longer_exists() {
        let source = Fake::new(descriptor(), entries());

        assert_focused_matches_unified(
            &source,
            AgentId::Amp,
            &shared(None, None),
            &every_kind_diverges(),
        );
    }
}
