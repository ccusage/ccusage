use crate::{LoadedEntry, Result};
use ccusage_adapter_common::{source::LoadContext, units::read_typed_units_flat};

use super::paths::discover_files;

pub(crate) fn load_entries_for_context(cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
    let shared = cx.shared;
    let pricing = cx.pricing.table(cx.shared.mode);
    let sources = discover_files(cx.diagnostics)?;
    let timezone = cx.window.timezone();
    let mut entries =
        read_typed_units_flat(&sources, shared.single_thread, cx.diagnostics, |source| {
            super::parser::read_file(source, shared.mode, pricing, timezone, cx.diagnostics)
        });
    entries.sort_by(|left, right| {
        left.timestamp
            .cmp(&right.timestamp)
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use crate::cli::CostMode;
    use crate::cli::SharedArgs;
    use ccusage_core::PricingMap;
    use ccusage_test_support::{Environment, Fixture};

    #[test]
    fn diagnoses_untimed_extension_and_cli_usage() {
        let fixture = ccusage_test_support::fs_fixture!({
            "tasks/task-a/ui_messages.json": r#"[
                {"type":"say","say":"api_req_started","modelInfo":{"modelId":"test-model"},"text":"{\"tokensIn\":1,\"tokensOut\":0,\"cacheReads\":0,\"cacheWrites\":0}"},
                {"type":"say","say":"api_req_started","ts":0,"modelInfo":{"modelId":"test-model"},"text":"{\"tokensIn\":1,\"tokensOut\":0,\"cacheReads\":0,\"cacheWrites\":0}"}
            ]"#,
            "data/sessions/session-a/session-a.messages.json": r#"{"messages":[
                {"role":"assistant","modelInfo":{"id":"test-model"},"metrics":{"inputTokens":1,"outputTokens":0,"cacheReadTokens":0,"cacheWriteTokens":0}},
                {"role":"assistant","metrics":{}}
            ]}"#
        });
        Environment::with_var(super::super::paths::CLINE_DATA_DIR_ENV, fixture.root()).run(|| {
            let (entries, diagnostics) = ccusage_adapter_common::testing::load_entries(
                &crate::SOURCE,
                &SharedArgs::default(),
                ccusage_adapter_common::source::SourceOptions::default(),
                Some(&PricingMap::default()),
            )
            .unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].timestamp, crate::TimestampMs::UNIX_EPOCH);
            assert_eq!(
                diagnostics.skipped(ccusage_core::SkipKind::InvalidTimestamp),
                2
            );
        })
    }

    #[test]
    fn loads_extension_and_cli_fixture_records_with_source_namespaced_sessions() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = Fixture::new();
        assert!(
            fixture
                .write_file(
                    "tasks/session-a/ui_messages.json",
                    include_str!("../tests/fixtures/extension/ui_messages.json"),
                )
                .is_file()
        );
        assert!(
            fixture
                .write_file(
                    "tasks/session-a/task_metadata.json",
                    include_str!("../tests/fixtures/extension/task_metadata.json"),
                )
                .is_file()
        );
        assert!(
            fixture
                .write_file(
                    "tasks/session-a/api_conversation_history.json",
                    include_str!("../tests/fixtures/extension/api_conversation_history.json"),
                )
                .is_file()
        );
        assert!(
            fixture
                .write_file(
                    "data/sessions/session-a/session-a.messages.json",
                    include_str!("../tests/fixtures/cli/session.messages.json"),
                )
                .is_file()
        );
        assert!(
            fixture
                .write_file(
                    "data/tasks/session-a/ui_messages.json",
                    include_str!("../tests/fixtures/extension/legacy_cli_ui_messages.json"),
                )
                .is_file()
        );
        Environment::with_var(
            super::super::paths::CLINE_DATA_DIR_ENV,
            format!(
                "{},{}",
                fixture.root().display(),
                fixture.root().join("data").display()
            ),
        )
        .run(|| {
            let shared = SharedArgs {
                json: true,
                single_thread: true,
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                mode: CostMode::Auto,
                ..SharedArgs::default()
            };

            let entries = ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &shared,
                Default::default(),
                Some(&PricingMap::load_embedded()),
            )
            .unwrap();

            assert_eq!(
                entries.len(),
                4,
                "only fully attributable requests with complete usage are records"
            );
            assert_eq!(
                entries
                    .iter()
                    .map(|entry| entry.date.to_string())
                    .collect::<Vec<_>>(),
                ["2026-10-01", "2026-11-01", "2026-11-01", "2026-11-01"]
            );
            assert_eq!(
                entries
                    .iter()
                    .map(|entry| entry.session_id.as_ref())
                    .collect::<Vec<_>>(),
                [
                    "task:session-a",
                    "cli:session-a",
                    "cli:session-a",
                    "task:session-a"
                ]
            );
            assert_eq!(
                entries[0].model.as_deref(),
                Some("claude-sonnet-4-20250514")
            );
            assert_eq!(
                entries[1].model.as_deref(),
                Some("claude-sonnet-4-20250514")
            );
            assert_eq!(entries[1].usage.cache_read_input_tokens, 25);
            assert_eq!(entries[1].usage.cache_creation_input_tokens, 10);
            assert_eq!(entries[1].message_id.as_deref(), Some("assistant-1"));
            assert_eq!(entries[1].usage.input_tokens, 65);
            assert_eq!(entries[1].recorded_cost, Some(0.0));
            assert_eq!(entries[1].stored_cost, Some(0.0));
            assert_eq!(entries[2].message_id.as_deref(), Some("assistant-2"));
            assert_eq!(entries[2].usage.input_tokens, 90);
            assert_eq!(entries[2].usage.output_tokens, 16);
            assert_eq!(entries[2].recorded_cost, Some(0.0002));
            assert_eq!(entries[3].usage.input_tokens, 8);
            assert_eq!(entries[3].usage.output_tokens, 4);
            assert_eq!(entries[3].model.as_deref(), Some("vendor/legacy-model"));
            assert_eq!(
                entries[3].missing_pricing_model.as_deref(),
                Some("vendor/legacy-model")
            );
        })
    }

    #[test]
    fn keeps_healthy_cline_files_when_another_file_is_invalid_json() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = Fixture::new();
        fixture.write_file(
            "tasks/healthy/ui_messages.json",
            include_str!("../tests/fixtures/extension/ui_messages.json"),
        );
        fixture.write_file("tasks/damaged/ui_messages.json", "{\"type\":}");
        fixture.write_file("tasks/unwritten/ui_messages.json", "");
        Environment::with_var(super::super::paths::CLINE_DATA_DIR_ENV, fixture.root()).run(|| {
            let shared = SharedArgs {
                single_thread: true,
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                ..SharedArgs::default()
            };

            let (entries, diagnostics) = ccusage_adapter_common::testing::load_entries(
                &crate::SOURCE,
                &shared,
                ccusage_adapter_common::source::SourceOptions::default(),
                Some(&PricingMap::load_embedded()),
            )
            .unwrap();

            assert!(!entries.is_empty());
            assert!(
                entries
                    .iter()
                    .all(|entry| entry.session_id.as_ref() == "task:healthy")
            );
            assert_eq!(
                diagnostics.skipped(ccusage_core::SkipKind::InvalidData),
                1,
                "the damaged task is reported, the unwritten one is not"
            );
        })
    }
}
