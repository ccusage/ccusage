use ccusage_adapter_common::{
    claims::{ClaudeClaimsTx, ClaudeCounted},
    rows::{summarize_entry_sessions, summary_rows},
    source::{LoadKind, LoadRequest, LoadedRows, PricingSupply, UsageSource},
};
use ccusage_core::{Diagnostics, ReportWindow, Result};

use crate::{load_daily_summaries_with_detection_progress, load_deduped_entries_with_progress};

pub struct Claude;

pub static SOURCE: Claude = Claude;

impl Claude {
    pub fn counted(
        &self,
        shared: &ccusage_core::cli::SharedArgs,
        diagnostics: &Diagnostics,
    ) -> Result<ClaudeCounted> {
        let window = ReportWindow::from_shared(shared);
        let deduped = crate::load_entries_with_mode(
            shared,
            None,
            false,
            diagnostics,
            ccusage_core::cli::CostMode::Display,
        )?;
        Ok(ClaudeCounted::session_days(
            deduped.main_thread_session_days(|entry| window.contains_date(&entry.date)),
            shared,
        ))
    }

    pub fn load_with_claims(
        &self,
        request: &LoadRequest<'_>,
        claims: ClaudeClaimsTx,
    ) -> Result<LoadedRows> {
        let (rows, counted) = self.load(request)?;
        claims.publish(counted);
        Ok(rows)
    }

    fn load(&self, request: &LoadRequest<'_>) -> Result<(LoadedRows, ClaudeCounted)> {
        let shared = request.shared;
        let window = ReportWindow::from_shared(shared);
        let project_filter = request.options.claude_project;
        let diagnostics = Diagnostics::new("claude");
        if request.kind == LoadKind::Session {
            let deduped = load_deduped_entries_with_progress(
                shared,
                project_filter,
                request.progress,
                &diagnostics,
            )?;
            let detected = !deduped.entries.is_empty();
            let sessions = deduped
                .main_thread_session_days(|entry| window.contains_date(&entry.date))
                .into_iter()
                .map(|(session, _)| session)
                .collect();
            let entries = deduped
                .entries
                .into_iter()
                .filter(|entry| window.contains_date(&entry.date))
                .collect::<Vec<_>>();
            let summaries = summarize_entry_sessions(&entries)?;
            return Ok((
                LoadedRows {
                    rows: summary_rows("claude", summaries, false),
                    detected,
                    diagnostics,
                },
                ClaudeCounted::sessions(sessions, shared),
            ));
        }
        let loaded = load_daily_summaries_with_detection_progress(
            shared,
            project_filter,
            false,
            request.progress,
            &diagnostics,
        )?;
        let summaries = loaded
            .summaries
            .into_iter()
            .filter(|summary| {
                window.is_unbounded()
                    || window.contains_date(summary.date.as_deref().unwrap_or_default())
            })
            .collect();
        let session_days = loaded
            .session_days
            .into_iter()
            .filter(|(_, date)| window.contains_date(date))
            .collect();
        Ok((
            LoadedRows {
                rows: summary_rows("claude", summaries, false),
                detected: loaded.detected,
                diagnostics,
            },
            ClaudeCounted::session_days(session_days, shared),
        ))
    }
}

impl UsageSource for Claude {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn label(&self) -> &'static str {
        "Claude"
    }

    fn pricing_supply(&self) -> PricingSupply {
        PricingSupply::Source
    }

    fn load_rows(&self, request: &LoadRequest<'_>) -> Result<LoadedRows> {
        self.load(request).map(|(rows, _)| rows)
    }
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;
    use ccusage_adapter_common::source::{ProgressOwner, SourceOptions};
    use ccusage_core::cli::{AgentReportKind, CostMode, SharedArgs};
    use ccusage_test_support::{EnvVarsGuard, Fixture};

    #[test]
    fn counted_reads_no_pricing_and_retains_only_in_window_main_thread_days() {
        let fixture = Fixture::new();
        let path = fixture.write_file("projects/p/session-main.jsonl", r#"{"timestamp":"2026-01-02T00:00:00Z","sessionId":"session-main","message":{"id":"msg-main","model":"claude-opus-5","usage":{"input_tokens":10,"output_tokens":1}}}"#);
        let _env = EnvVarsGuard::set_many([(
            "CLAUDE_CONFIG_DIR",
            Some(fixture.root().as_os_str().to_owned()),
        )]);
        let shared = SharedArgs {
            mode: CostMode::Calculate,
            timezone: Some("UTC".to_owned()),
            ..Default::default()
        };
        let diagnostics = Diagnostics::new("claude");
        let counted = SOURCE.counted(&shared, &diagnostics).unwrap();
        let entry = crate::read_usage_file_with_diagnostics(
            &path,
            ReportWindow::from_shared(&shared).timezone(),
            CostMode::Display,
            None,
            &diagnostics,
        )
        .entries
        .remove(0)
        .into_loaded();
        assert!(
            counted
                .retain_uncounted(vec![(entry, Some(std::sync::Arc::from("session-main")))])
                .is_empty()
        );
    }

    #[test]
    fn specialized_diagnostics_keep_healthy_claude_usage() {
        let fixture = Fixture::new();
        let _ = fixture.write_file("projects/p/good.jsonl", r#"{"timestamp":"2026-01-02T00:00:00Z","message":{"id":"good","model":"claude-opus-5","usage":{"input_tokens":10,"output_tokens":1}}}"#);
        let _ = fixture.write_file("projects/p/bad.jsonl", r#"{"timestamp":"2026-01-02T00:00:00Z","message":{"usage":{"input_tokens":"SECRET_CANARY"}}}"#);
        let _env = EnvVarsGuard::set_many([
            (
                "CLAUDE_CONFIG_DIR",
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
        let diagnostics = Diagnostics::new("claude");
        let focused = crate::load_deduped_entries_with_progress(
            &shared,
            None,
            ProgressOwner::Caller,
            &diagnostics,
        )
        .unwrap();
        assert_eq!(focused.entries.len(), 1);
        assert!(
            diagnostics
                .warnings(&shared)
                .iter()
                .any(|line| line.contains("invalid data"))
        );
        for kind in [AgentReportKind::Daily, AgentReportKind::Session] {
            let rows = SOURCE
                .load_rows(&LoadRequest {
                    kind: ccusage_adapter_common::source::LoadKind::for_report(kind),
                    shared: &shared,
                    options: SourceOptions::default(),
                    pricing: None,
                    progress: ProgressOwner::Caller,
                })
                .unwrap();
            assert_eq!(rows.rows.len(), 1);
            let warnings = rows.diagnostics.warnings(&shared);
            assert!(
                warnings.iter().any(|line| line.contains("invalid data")),
                "{warnings:?}"
            );
            assert!(!warnings.join("\n").contains("SECRET_CANARY"));
        }
    }
}
