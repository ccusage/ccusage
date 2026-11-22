use std::borrow::Cow;

use ccusage_adapter_common::{
    claims::{ClaudeClaimsTx, ClaudeCounted},
    rows::{summarize_entry_sessions, summary_rows},
    source::{LoadKind, LoadRequest, LoadedRows, UsageSource},
};
use ccusage_core::{Diagnostics, ReportWindow, Result, cli::SharedArgs};

use crate::load_daily_summaries_with_detection_progress;

pub struct Claude;

pub static SOURCE: Claude = Claude;

impl Claude {
    pub fn counted(
        &self,
        shared: &ccusage_core::cli::SharedArgs,
        project_filter: Option<&str>,
        diagnostics: &Diagnostics,
    ) -> Result<ClaudeCounted> {
        let load_shared = claim_load_options(shared);
        let deduped = crate::load_windowed_entries(
            &load_shared,
            project_filter,
            diagnostics,
            ccusage_core::cli::CostMode::Display,
        )?;
        Ok(ClaudeCounted::sessions(
            deduped.main_thread_sessions(),
            shared,
        ))
    }

    pub fn load_with_claims(
        &self,
        request: &LoadRequest<'_>,
        claims: ClaudeClaimsTx,
    ) -> Result<LoadedRows> {
        let (rows, counted) = self.load(request, true)?;
        // NOTE(2026-11-22, #2498): Project filters hide Claude rows. Checking every project prevents excluded sessions from reappearing under OpenCode.
        let counted = if request.options.claude_project.is_some() {
            match request.kind {
                LoadKind::Day => {
                    let shared = claim_load_options(request.shared);
                    let daily = crate::daily::load_daily_entries(&shared, None, &rows.diagnostics)?;
                    ClaudeCounted::sessions(daily.sessions(), request.shared)
                }
                LoadKind::Session => self.counted(request.shared, None, &rows.diagnostics)?,
            }
        } else {
            counted
        };
        claims.publish(counted);
        Ok(rows)
    }

    fn load(
        &self,
        request: &LoadRequest<'_>,
        with_claims: bool,
    ) -> Result<(LoadedRows, ClaudeCounted)> {
        let shared = request.shared;
        let load_shared = if with_claims {
            claim_load_options(shared)
        } else {
            Cow::Borrowed(shared)
        };
        let report_window =
            matches!(load_shared, Cow::Owned(_)).then(|| ReportWindow::from_shared(shared));
        let project_filter = request.options.claude_project;
        let diagnostics = Diagnostics::new("claude");
        if request.kind == LoadKind::Session {
            let mut deduped = request.progress.run(
                ccusage_core::progress::UsageLoadAgent("Claude"),
                shared.json,
                || {
                    crate::load_windowed_entries(
                        &load_shared,
                        project_filter,
                        &diagnostics,
                        shared.mode,
                    )
                },
            )?;
            let detected = deduped.detected;
            let sessions = deduped.main_thread_sessions();
            if let Some(window) = report_window.as_ref() {
                crate::retain_copied_entries(
                    &mut deduped.entries,
                    &mut deduped.copies,
                    window,
                    |entry| entry.date,
                );
            }
            let entries = deduped
                .entries
                .into_iter()
                .map(|entry| entry.entry)
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
            &load_shared,
            project_filter,
            report_window.as_ref(),
            request.progress,
            &diagnostics,
        )?;
        let sessions = loaded.sessions;
        Ok((
            LoadedRows {
                rows: summary_rows("claude", loaded.summaries, false),
                detected: loaded.detected,
                diagnostics,
            },
            ClaudeCounted::sessions(sessions, shared),
        ))
    }
}

fn claim_load_options(shared: &SharedArgs) -> Cow<'_, SharedArgs> {
    if shared.until.is_none() {
        return Cow::Borrowed(shared);
    }
    Cow::Owned(SharedArgs {
        until: ReportWindow::from_shared(shared).next_day_until(),
        ..shared.clone()
    })
}

impl UsageSource for Claude {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn label(&self) -> &'static str {
        "Claude"
    }

    fn uses_shared_pricing(&self) -> bool {
        false
    }

    fn load_rows(&self, request: &LoadRequest<'_>) -> Result<LoadedRows> {
        self.load(request, false).map(|(rows, _)| rows)
    }
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;
    use ccusage_adapter_common::source::{ProgressOwner, SourceOptions};
    use ccusage_core::ReportWindow;
    use ccusage_core::cli::{AgentReportKind, CostMode, SharedArgs};
    use ccusage_test_support::{Environment, Fixture};
    use std::sync::Arc;

    #[test]
    fn claims_accept_the_last_supported_report_day() {
        let fixture = Fixture::new();
        fixture.create_dir_all("projects");
        Environment::with_vars([(
            "CLAUDE_CONFIG_DIR",
            Some(fixture.root().as_os_str().to_owned()),
        )])
        .run(|| {
            let shared = SharedArgs {
                since: Some("99991231".parse().unwrap()),
                until: Some("99991231".parse().unwrap()),
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                ..Default::default()
            };
            assert!(
                SOURCE
                    .counted(&shared, None, &Diagnostics::new("claude"))
                    .is_ok()
            );
        })
    }

    #[test]
    fn counted_matches_main_thread_sessions_inside_the_source_window() {
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
        fixture.write_file(
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
        Environment::with_vars([(
            "CLAUDE_CONFIG_DIR",
            Some(fixture.root().as_os_str().to_owned()),
        )])
        .run(|| {
            let shared = SharedArgs {
                mode: CostMode::Calculate,
                since: Some("20260101".parse().unwrap()),
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                ..Default::default()
            };
            let diagnostics = Diagnostics::new("claude");
            let counted = SOURCE.counted(&shared, None, &diagnostics).unwrap();
            let projects_root = fixture.root().join("projects");
            let probe = crate::load::read_usage_file_with_diagnostics(
                &main,
                Some(&projects_root),
                ReportWindow::from_shared(&shared).timezone(),
                ccusage_core::cost::PriceTable::Display,
                &diagnostics,
            )
            .remove(0);
            let at = |timestamp: &str| {
                let mut entry = probe.clone();
                entry.entry.timestamp = crate::parse_ts_timestamp(timestamp).unwrap();
                entry
            };
            let probes = [
                ("session-main", "2026-01-02T12:00:00Z", true),
                ("session-resumed", "2026-01-02T12:00:00Z", true),
                ("session-side", "2026-01-02T12:00:00Z", false),
                ("session-main", "2026-01-03T12:00:00Z", true),
                ("session-main", "2025-12-01T12:00:00Z", false),
                ("session-stale", "2026-01-03T12:00:00Z", true),
            ];
            for (session, timestamp, is_counted) in probes {
                let kept =
                    counted.retain_uncounted(vec![(at(timestamp).entry, Some(Arc::from(session)))]);
                assert_eq!(kept.is_empty(), is_counted, "{session} {timestamp}");
            }
            assert!(diagnostics.warnings(&shared).is_empty());
        })
    }

    #[test]
    fn specialized_diagnostics_keep_healthy_claude_usage() {
        let fixture = Fixture::new();
        fixture.write_file("projects/p/good.jsonl", r#"{"timestamp":"2026-01-02T00:00:00Z","message":{"id":"good","model":"claude-opus-5","usage":{"input_tokens":10,"output_tokens":1}}}"#);
        fixture.write_file("projects/p/bad.jsonl", r#"{"timestamp":"2026-01-02T00:00:00Z","message":{"usage":{"input_tokens":"SECRET_CANARY"}}}"#);
        Environment::with_vars([
            (
                "CLAUDE_CONFIG_DIR",
                Some(fixture.root().as_os_str().to_owned()),
            ),
            ("LOG_LEVEL", Some("3".into())),
        ])
        .run(|| {
            let shared = SharedArgs {
                offline: true,
                mode: CostMode::Display,
                debug: true,
                ..Default::default()
            };
            let diagnostics = Diagnostics::new("claude");
            let focused = crate::load::load_deduped_entries_with_progress(
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
        })
    }
}
