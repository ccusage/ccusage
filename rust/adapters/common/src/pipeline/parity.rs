use std::collections::BTreeMap;

use serde_json::Value;

use ccusage_core::cli::{AgentCommandArgs, AgentId, AgentReportKind, SharedArgs, WeekDay};

use super::{focused_rows, load_rows};
use crate::source::{EntrySource, LoadRequest, ProgressOwner, SourceOptions};

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

/// Compares focused and unified totals for every supported report kind.
/// The fixture must contain usage before date filtering.
pub fn assert_focused_matches_unified<S: EntrySource>(
    source: &S,
    agent: AgentId,
    shared: &SharedArgs,
) {
    let descriptor = source.descriptor();
    let unbounded = SharedArgs {
        since: None,
        until: None,
        ..shared.clone()
    };
    let everything = focused_totals(source, AgentReportKind::Daily, &unbounded);
    assert!(
        everything
            .as_ref()
            .is_ok_and(|totals| totals.total_tokens > 0 || totals.total_cost > 0.0),
        "{}: the focused report read no usage, so there is nothing to compare; point the source at its fixture first ({everything:?})",
        descriptor.id
    );
    assert_eq!(
        descriptor.id,
        agent.spec().name,
        "the source is not the agent whose reports it is checked for"
    );
    for kind in AgentReportKind::ALL {
        let focused = focused_totals(source, kind, shared);
        let unified = unified_totals(source, kind, shared);
        assert!(
            matches!((&focused, &unified), (Ok(focused), Ok(unified)) if focused.matches(unified)),
            "{} {kind:?}: focused totals {focused:?} differ from the unified slice {unified:?} (since {:?}, until {:?}, timezone {:?})",
            descriptor.id,
            shared.since,
            shared.until,
            shared.timezone
        );
    }
}

fn focused_totals<S: EntrySource>(
    source: &S,
    kind: AgentReportKind,
    shared: &SharedArgs,
) -> Result<Totals, String> {
    let args = AgentCommandArgs {
        options: Default::default(),
        shared: shared.clone(),
        kind,
        session_id: None,
        sections: None,
        by_agent: false,

        include_claude_agent: false,
        start_of_week: WeekDay::Sunday,
        sources: BTreeMap::new(),
    };
    let rows = focused_rows(source, &args)
        .map_err(|error| format!("focused {kind:?} load failed: {error}"))?;
    let report = source.report_json(&rows, kind);
    let totals = &report["totals"];
    let tokens = |key: &str| totals.get(key).and_then(Value::as_u64).unwrap_or(0);
    Ok(Totals {
        input_tokens: tokens("inputTokens"),
        output_tokens: tokens("outputTokens"),
        cache_creation_tokens: tokens("cacheCreationTokens"),
        cache_read_tokens: tokens("cacheReadTokens"),
        total_tokens: tokens("totalTokens"),
        total_cost: totals
            .get("totalCost")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
    })
}

fn unified_totals<S: EntrySource>(
    source: &S,
    kind: AgentReportKind,
    shared: &SharedArgs,
) -> Result<Totals, String> {
    let load_kind = crate::source::LoadKind::for_report(kind).report_kind();
    let rows = load_rows(
        source,
        &LoadRequest {
            kind: crate::source::LoadKind::for_report(load_kind),
            shared,
            options: SourceOptions::default(),
            pricing: None,
            progress: ProgressOwner::Caller,
        },
    )
    .map_err(|error| format!("unified {kind:?} load failed: {error}"))?
    .rows;
    let sum = |tokens: fn(&crate::rows::SourceRow) -> u64| {
        rows.iter().map(tokens).fold(0, u64::saturating_add)
    };
    Ok(Totals {
        input_tokens: sum(|row| row.input_tokens),
        output_tokens: sum(|row| row.output_tokens),
        cache_creation_tokens: sum(|row| row.cache_creation_tokens),
        cache_read_tokens: sum(|row| row.cache_read_tokens),
        total_tokens: sum(|row| row.total_tokens),
        total_cost: rows.iter().map(|row| row.total_cost).sum(),
    })
}

#[cfg(test)]
mod tests {
    use ccusage_core::cli::AgentId;

    use super::assert_focused_matches_unified;
    use crate::test_source::{Fake, entries, shared};

    fn descriptor() -> crate::source::EntryDescriptor {
        crate::source::EntryDescriptor {
            id: "amp",
            ..crate::test_source::descriptor()
        }
    }

    #[test]
    fn passes_a_source_whose_reports_agree() {
        let source = Fake::new(descriptor(), entries());

        assert_focused_matches_unified(&source, AgentId::Amp, &shared(None, None));
        assert_focused_matches_unified(&source, AgentId::Amp, &shared(Some("20260102"), None));
    }

    #[test]
    fn compares_a_window_that_leaves_nothing() {
        let source = Fake::new(descriptor(), entries());

        assert_focused_matches_unified(&source, AgentId::Amp, &shared(Some("20300101"), None));
    }

    #[test]
    #[should_panic(expected = "the focused report read no usage")]
    fn fails_a_source_that_reads_no_usage() {
        let source = Fake::new(descriptor(), Vec::new());

        assert_focused_matches_unified(&source, AgentId::Amp, &shared(None, None));
    }

    #[test]
    #[should_panic(expected = "differ from the unified slice")]
    fn fails_when_the_loader_changes_between_reports() {
        use crate::source::{EntryDescriptor, EntrySource, LoadContext};
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct ChangingSource(AtomicUsize);
        impl EntrySource for ChangingSource {
            fn descriptor(&self) -> EntryDescriptor {
                descriptor()
            }
            fn load_entries(
                &self,
                _cx: &LoadContext<'_>,
            ) -> ccusage_core::Result<Vec<ccusage_core::LoadedEntry>> {
                let mut loaded = entries();
                if self.0.fetch_add(1, Ordering::Relaxed) > 1 {
                    loaded.remove(0);
                }
                Ok(loaded)
            }
        }
        assert_focused_matches_unified(
            &ChangingSource(AtomicUsize::new(0)),
            AgentId::Amp,
            &shared(None, None),
        );
    }
}
