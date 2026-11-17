use ccusage_adapter_common::{
    claims::{ClaudeClaimsTx, ClaudeCounted},
    rows::{summarize_entry_sessions, summary_rows},
    source::{LoadKind, LoadRequest, LoadedRows, PricingSupply, UsageSource},
};
use ccusage_core::{Diagnostics, Result};

use crate::load_daily_summaries_with_detection_progress;

pub struct Claude;

pub static SOURCE: Claude = Claude;

impl Claude {
    /// The main-thread session days this source counts inside `shared`'s
    /// window, for every focused report kind: a focused session report counts
    /// only a session's in-window days, so it matches by session day too.
    pub fn counted(
        &self,
        shared: &ccusage_core::cli::SharedArgs,
        project_filter: Option<&str>,
        diagnostics: &Diagnostics,
    ) -> Result<ClaudeCounted> {
        let deduped = crate::load_windowed_entries(
            shared,
            project_filter,
            diagnostics,
            ccusage_core::cli::CostMode::Display,
        )?;
        Ok(ClaudeCounted::session_days(
            deduped.main_thread_session_days(|_| true),
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
        let project_filter = request.options.claude_project;
        let diagnostics = Diagnostics::new("claude");
        if request.kind == LoadKind::Session {
            let deduped = request.progress.run(
                ccusage_core::progress::UsageLoadAgent("Claude"),
                shared.json,
                || crate::load_windowed_entries(shared, project_filter, &diagnostics, shared.mode),
            )?;
            let detected = deduped.detected;
            let sessions = deduped
                .main_thread_session_days(|_| true)
                .into_iter()
                .map(|(session, _)| session)
                .collect();
            let summaries = summarize_entry_sessions(&deduped.entries)?;
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
        let summaries = loaded.summaries;
        let session_days = loaded.session_days;
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
    use ccusage_core::ReportWindow;
    use ccusage_core::cli::{AgentReportKind, CostMode, SharedArgs};
    use ccusage_test_support::{EnvVarsGuard, Fixture};
    use std::sync::Arc;

    #[test]
    fn counted_keeps_only_in_window_main_thread_session_days() {
        let fixture = Fixture::new();
        let main = fixture.write_file(
            "projects/p/session-main.jsonl",
            [
                r#"{"timestamp":"2026-01-02T00:00:00Z","sessionId":"session-main","requestId":"req-main","message":{"id":"msg-main","model":"claude-opus-5","usage":{"input_tokens":10,"output_tokens":1}}}"#,
                r#"{"timestamp":"2026-01-02T01:00:00Z","sessionId":"session-side","requestId":"req-side","isSidechain":true,"message":{"id":"msg-side","model":"claude-opus-5","usage":{"input_tokens":10,"output_tokens":1}}}"#,
                r#"{"timestamp":"2025-12-01T00:00:00Z","sessionId":"session-main","requestId":"req-old","message":{"id":"msg-old","model":"claude-opus-5","usage":{"input_tokens":10,"output_tokens":1}}}"#,
            ]
            .join("\n"),
        );
        let _ = fixture.write_file(
            "projects/p/session-resumed.jsonl",
            r#"{"timestamp":"2026-01-02T00:00:00Z","sessionId":"session-resumed","requestId":"req-main","message":{"id":"msg-main","model":"claude-opus-5","usage":{"input_tokens":10,"output_tokens":1}}}"#,
        );
        let stale = fixture.write_file(
            "projects/p/session-stale.jsonl",
            r#"{"timestamp":"2026-01-03T00:00:00Z","sessionId":"session-stale","requestId":"req-stale","message":{"id":"msg-stale","model":"claude-opus-5","usage":{"input_tokens":10,"output_tokens":1}}}"#,
        );
        ccusage_test_support::set_modified(
            &stale,
            ccusage_test_support::instant("2025-06-01T00:00:00Z"),
        );
        let _env = EnvVarsGuard::set_many([(
            "CLAUDE_CONFIG_DIR",
            Some(fixture.root().as_os_str().to_owned()),
        )]);
        let shared = SharedArgs {
            mode: CostMode::Calculate,
            since: Some("20260101".to_owned()),
            timezone: Some("UTC".to_owned()),
            ..Default::default()
        };
        let diagnostics = Diagnostics::new("claude");
        let counted = SOURCE.counted(&shared, None, &diagnostics).unwrap();
        let probe = crate::read_usage_file_with_diagnostics(
            &main,
            ReportWindow::from_shared(&shared).timezone(),
            CostMode::Display,
            None,
            &diagnostics,
        )
        .remove(0);
        let at = |timestamp: &str| {
            let mut entry = probe.clone();
            entry.timestamp = crate::parse_ts_timestamp(timestamp).unwrap();
            entry
        };
        let probes = [
            ("session-main", "2026-01-02T12:00:00Z", true),
            ("session-resumed", "2026-01-02T12:00:00Z", true),
            ("session-side", "2026-01-02T12:00:00Z", false),
            ("session-main", "2026-01-03T12:00:00Z", false),
            ("session-main", "2025-12-01T12:00:00Z", false),
            ("session-stale", "2026-01-03T12:00:00Z", false),
        ];
        for (session, timestamp, is_counted) in probes {
            let kept = counted.retain_uncounted(vec![(at(timestamp), Some(Arc::from(session)))]);
            assert_eq!(kept.is_empty(), is_counted, "{session} {timestamp}");
        }
        assert!(diagnostics.warnings(&shared).is_empty());
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
