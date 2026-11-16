use ccusage_adapter_common::{
    claims::ClaudeCounted,
    rows::summary_rows,
    source::{ALL_REPORTS, PricingSupply, ProgressOwner, SourceRows, UnifiedRequest, UsageSource},
};
use ccusage_core::Diagnostics;
use ccusage_core::{
    ReportWindow, Result,
    cli::AgentReportKind,
    progress::{UsageLoadAgent, track_usage_load},
};

use crate::{
    has_data, loader::load_entries_with_native_sessions_with_diagnostics, summarize_entries,
};

pub struct OpenCode;

pub static SOURCE: OpenCode = OpenCode;

impl UsageSource for OpenCode {
    fn id(&self) -> &'static str {
        "opencode"
    }

    fn label(&self) -> &'static str {
        "OpenCode"
    }

    fn reports(&self) -> &'static [AgentReportKind] {
        ALL_REPORTS
    }

    fn pricing_supply(&self) -> PricingSupply {
        PricingSupply::Source
    }

    fn unified_rows(&self, request: &UnifiedRequest<'_>) -> Result<SourceRows> {
        let load = || unified_rows(request);
        match request.progress {
            ProgressOwner::Source => {
                track_usage_load(UsageLoadAgent("OpenCode"), request.shared.json, load)
            }
            ProgressOwner::Caller => load(),
        }
    }
}

fn unified_rows(request: &UnifiedRequest<'_>) -> Result<SourceRows> {
    let diagnostics = Diagnostics::new("opencode");
    let linked = load_entries_with_native_sessions_with_diagnostics(
        request.shared,
        request.kind,
        ProgressOwner::Caller,
        &diagnostics,
    )?;
    let detected = !linked.is_empty() || has_data();
    let mut entries = if linked.iter().any(|(_, session)| session.is_some()) {
        request
            .claude_counted
            .map_or_else(ClaudeCounted::nothing, |load| load())
            .retain_uncounted(linked)
    } else {
        linked.into_iter().map(|(entry, _)| entry).collect()
    };
    let window = ReportWindow::from_shared(request.shared);
    entries.retain(|entry| window.contains_date(&entry.date));
    let summaries = summarize_entries(&entries, request.kind)?;

    Ok(SourceRows {
        rows: summary_rows("opencode", summaries, false),
        detected,
        diagnostics,
        claude_counted: None,
    })
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;
    use ccusage_adapter_common::source::SourceOptions;
    use ccusage_core::cli::{CostMode, SharedArgs};
    use ccusage_test_support::{EnvVarsGuard, Fixture};
    #[test]
    fn specialized_diagnostics_keep_healthy_opencode_usage() {
        let fixture = Fixture::new();
        let _ = fixture.write_file("opencode.db", "SECRET_CANARY");
        let _ = fixture.write_file("storage/message/good.json", r#"{"id":"good","sessionID":"s","modelID":"claude-opus-5","providerID":"anthropic","time":{"created":1767312000000},"tokens":{"input":10,"output":1}}"#);
        let _ = fixture.write_file("storage/message/bad.json", "{SECRET_CANARY");
        let _env = EnvVarsGuard::set_many([
            (
                "OPENCODE_DATA_DIR",
                Some(fixture.root().as_os_str().to_owned()),
            ),
            ("LOG_LEVEL", Some("3".into())),
        ]);
        let shared = SharedArgs {
            offline: true,
            mode: CostMode::Display,
            debug: true,
            ..Default::default()
        };
        let rows = SOURCE
            .unified_rows(&UnifiedRequest {
                kind: AgentReportKind::Daily,
                shared: &shared,
                options: SourceOptions::default(),
                pricing: None,
                progress: ProgressOwner::Caller,
                multi_source_driver: true,
                claude_counted: None,
            })
            .unwrap();
        assert_eq!(rows.rows.len(), 1);
        assert!(
            rows.diagnostics
                .warnings(&shared)
                .iter()
                .any(|line| line.contains("invalid data")),
            "{:?}",
            rows.diagnostics.warnings(&shared)
        );
        assert!(
            rows.diagnostics
                .warnings(&shared)
                .iter()
                .any(|line| line.contains("unreadable")),
            "{:?}",
            rows.diagnostics.warnings(&shared)
        );
        assert!(
            !rows
                .diagnostics
                .warnings(&shared)
                .join("\n")
                .contains("SECRET_CANARY")
        );
    }
}
