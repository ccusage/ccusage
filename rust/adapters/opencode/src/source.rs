use ccusage_adapter_common::{
    claims::{ClaudeClaimsRx, ClaudeCounted, NativeSessions, native_sessions},
    rows::summary_rows,
    source::{LoadRequest, LoadedRows, ProgressOwner, UsageSource},
};
use ccusage_core::Diagnostics;
use ccusage_core::{Result, progress::UsageLoadAgent};

use crate::{
    has_data, loader::load_entries_with_native_sessions_with_diagnostics, summarize_entries,
};

pub struct OpenCode;

pub static SOURCE: OpenCode = OpenCode;

impl OpenCode {
    pub fn load_with_claims(
        &self,
        request: &LoadRequest<'_>,
        claims: ClaudeClaimsRx,
    ) -> Result<LoadedRows> {
        request
            .progress
            .run(UsageLoadAgent("OpenCode"), request.shared.json, || {
                load_rows(request, |sessions| claims.receive(sessions))
            })
    }
}

impl UsageSource for OpenCode {
    fn id(&self) -> &'static str {
        "opencode"
    }

    fn label(&self) -> &'static str {
        "OpenCode"
    }

    fn uses_shared_pricing(&self) -> bool {
        false
    }

    fn load_rows(&self, request: &LoadRequest<'_>) -> Result<LoadedRows> {
        request
            .progress
            .run(UsageLoadAgent("OpenCode"), request.shared.json, || {
                load_rows(request, |_| Ok(ClaudeCounted::nothing()))
            })
    }
}

fn load_rows(
    request: &LoadRequest<'_>,
    counted: impl FnOnce(NativeSessions) -> Result<ClaudeCounted>,
) -> Result<LoadedRows> {
    let diagnostics = Diagnostics::new("opencode");
    let linked = load_entries_with_native_sessions_with_diagnostics(
        request.shared,
        request.kind.report_kind(),
        ProgressOwner::Caller,
        &diagnostics,
    )?;
    let detected = !linked.is_empty() || has_data();
    let sessions = native_sessions(&linked);
    let mut entries = if sessions.is_empty() {
        linked.into_iter().map(|(entry, _)| entry).collect()
    } else {
        counted(sessions)?.retain_uncounted(linked)
    };
    ccusage_adapter_common::filter_loaded_entries_by_date(&mut entries, request.shared);
    let summaries = summarize_entries(
        &entries,
        request.kind.report_kind(),
        crate::cli::WeekDay::Monday,
    )?;

    Ok(LoadedRows {
        rows: summary_rows("opencode", summaries, false),
        detected,
        diagnostics,
    })
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;
    use ccusage_adapter_common::source::SourceOptions;
    use ccusage_core::cli::{AgentReportKind, CostMode, SharedArgs};
    use ccusage_test_support::{Environment, Fixture};
    #[test]
    fn specialized_diagnostics_keep_healthy_opencode_usage() {
        let fixture = Fixture::new();
        fixture.write_file("opencode.db", "SECRET_CANARY");
        fixture.write_file("storage/message/good.json", r#"{"id":"good","sessionID":"s","modelID":"claude-opus-5","providerID":"anthropic","time":{"created":1767312000000},"tokens":{"input":10,"output":1}}"#);
        fixture.write_file("storage/message/bad.json", "{SECRET_CANARY");
        Environment::with_vars([
            (
                "OPENCODE_DATA_DIR",
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
            let rows = SOURCE
                .load_rows(&LoadRequest {
                    kind: ccusage_adapter_common::source::LoadKind::for_report(
                        AgentReportKind::Daily,
                    ),
                    shared: &shared,
                    options: SourceOptions::default(),
                    pricing: None,
                    progress: ProgressOwner::Caller,
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
                    .any(|line| line.contains("opencode.db: database is corrupt or invalid")),
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
        })
    }
}
