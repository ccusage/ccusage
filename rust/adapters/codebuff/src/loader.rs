use ccusage_core::cost::PriceTable;
use std::sync::Arc;

use jiff::tz::TimeZone as JiffTimeZone;

use super::{
    parser::{CodebuffEntry, load_chat_file, price_codebuff},
    paths::discover_chat_files,
};
use ccusage_adapter_common::{
    dedupe::dedupe_last_wins, source::LoadContext, units::read_units_flat,
};

use crate::{LoadedEntry, Result, date_at};

pub(crate) fn load_entries_with_context(cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
    let pricing = cx.pricing.table(cx.shared.mode);
    let tz = cx.window.timezone();
    let mut files = discover_chat_files(cx.diagnostics)?;
    files.sort();
    let loaded = read_units_flat(&files, cx.shared.single_thread, cx.diagnostics, |path| {
        load_chat_file(path, cx.diagnostics)
    });
    let mut entries = dedupe_last_wins(loaded, |entry| entry.dedup_key.clone());
    entries.sort_by(|left, right| {
        (left.timestamp, &left.dedup_key).cmp(&(right.timestamp, &right.dedup_key))
    });
    Ok(entries
        .into_iter()
        .map(|entry| to_loaded_entry(entry, tz, pricing))
        .collect())
}

fn to_loaded_entry(
    entry: CodebuffEntry,
    tz: &JiffTimeZone,
    pricing: PriceTable<'_>,
) -> LoadedEntry {
    let (cost, missing_pricing_model, estimated_pricing) = price_codebuff(&entry, pricing);
    LoadedEntry {
        date: date_at(entry.timestamp, tz),
        timestamp: entry.timestamp,
        project: Arc::from("codebuff"),
        session_id: Arc::from(entry.session_id.as_str()),
        project_path: Arc::from("Codebuff"),
        cost,
        extra_total_tokens: entry.extra_total_tokens,
        credits: (entry.credits > 0.0).then_some(entry.credits),
        model: Some(entry.model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        estimated_pricing,
        message_count: None,
        usage: ccusage_core::TokenUsageRaw {
            output_tokens: entry.reported_output_tokens,
            ..entry.usage
        },
        message_id: Some(entry.dedup_key.clone()),
        version: None,
        recorded_cost: None,
        stored_cost: None,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn uniform_cost_modes_do_not_calculate_in_display() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({"projects/p/chats/c/chat-messages.json": r#"[{"role":"assistant","timestamp":"2026-05-01T01:02:03Z","metadata":{"model":"sample","usage":{"inputTokens":100,"outputTokens":50}}}]"#});
        Environment::with_var(CODEBUFF_DATA_DIR_ENV, fixture.root()).run(|| {
            let mut pricing = ccusage_core::PricingMap::default();
            pricing.load_json(
                r#"{"sample":{"input_cost_per_token":0.001,"output_cost_per_token":0.002}}"#,
            );
            for (mode, expected) in [
                (crate::cli::CostMode::Auto, 0.2),
                (crate::cli::CostMode::Calculate, 0.2),
                (crate::cli::CostMode::Display, 0.0),
            ] {
                let shared = SharedArgs {
                    mode,
                    ..SharedArgs::default()
                };
                let entries = ccusage_adapter_common::testing::entries(
                    &crate::SOURCE,
                    &shared,
                    Default::default(),
                    Some(&pricing),
                )
                .unwrap();
                assert_eq!(entries.len(), 1);
                assert!(
                    (entries[0].cost - expected).abs() < 1e-12,
                    "{mode:?}: {} vs {expected}",
                    entries[0].cost
                );
                assert_eq!(entries[0].missing_pricing_model, None);
            }
        })
    }

    use super::super::paths::CODEBUFF_DATA_DIR_ENV;
    use super::*;
    use crate::cli::SharedArgs;
    use crate::{TokenUsageRaw, cli::AgentReportKind, parse_ts_timestamp};
    use ccusage_adapter_common::parity::assert_focused_matches_unified;
    use ccusage_adapter_common::source::{LoadContext, SourceOptions};
    use ccusage_core::{Diagnostics, ReportWindow, SkipKind};
    use ccusage_test_support::{Environment, fs_fixture};

    #[test]
    fn tied_timestamps_are_ordered_by_dedup_key_after_last_wins() {
        let fixture = ccusage_test_support::Fixture::new();
        let path = "projects/project-a/chats/chat-a/chat-messages.json";
        let mut messages = (0..32)
            .rev()
            .map(|index| {
                serde_json::json!({
                    "id": format!("message-{index:02}"),
                    "role": "assistant",
                    "timestamp": "2026-01-02T03:04:06.000Z",
                    "metadata": {"model": "test-model", "usage": {"inputTokens": index + 1}}
                })
            })
            .collect::<Vec<_>>();
        messages.push(serde_json::json!({
            "id": "message-00", "role": "assistant",
            "timestamp": "2026-01-02T03:04:06.000Z",
            "metadata": {"model": "test-model", "usage": {"inputTokens": 99}}
        }));
        let _path = fixture.write_file(path, serde_json::to_string(&messages).unwrap());
        Environment::with_var(CODEBUFF_DATA_DIR_ENV, fixture.root()).run(|| {
            let pricing = ccusage_core::PricingMap::default();
            for single_thread in [true, false] {
                let entries = ccusage_adapter_common::testing::entries(
                    &crate::SOURCE,
                    &SharedArgs {
                        single_thread,
                        ..SharedArgs::default()
                    },
                    Default::default(),
                    Some(&pricing),
                )
                .unwrap();
                let ids = entries
                    .iter()
                    .map(|entry| {
                        entry
                            .message_id
                            .as_deref()
                            .unwrap()
                            .rsplit(':')
                            .next()
                            .unwrap()
                    })
                    .collect::<Vec<_>>();
                assert_eq!(
                    ids,
                    (0..32)
                        .map(|index| format!("message-{index:02}"))
                        .collect::<Vec<_>>()
                );
                assert_eq!(entries[0].usage.input_tokens, 99);
            }
        })
    }

    #[test]
    fn diagnoses_chat_records_only_when_all_timestamp_sources_fail() {
        let fixture = fs_fixture!({
            "projects/project-a/chats/chat-a/chat-messages.json": r#"[
                {"role":"assistant","metadata":{"usage":{"inputTokens":1}}},
                {"role":"assistant","timestamp":"invalid","metadata":{"usage":{"inputTokens":1}}},
                {"role":"assistant","timestamp":"1970-01-01T00:00:00Z","metadata":{"usage":{"inputTokens":1}}},
                {"role":"assistant","metadata":{"usage":{}}}
            ]"#
        });
        Environment::with_var(CODEBUFF_DATA_DIR_ENV, fixture.root()).run(|| {
            let (entries, diagnostics) = ccusage_adapter_common::testing::load_entries(
                &crate::SOURCE,
                &SharedArgs::default(),
                SourceOptions::default(),
                Some(&ccusage_core::PricingMap::default()),
            )
            .unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].timestamp, crate::TimestampMs::UNIX_EPOCH);
            assert_eq!(diagnostics.skipped(SkipKind::InvalidTimestamp), 2);
        })
    }

    #[test]
    fn ignores_file_mtime_for_an_untimed_chat_document() {
        let fixture = fs_fixture!({
            "projects/project-a/chats/chat-a/chat-messages.json": r#"[
                {"role":"assistant","metadata":{"usage":{"inputTokens":1}}}
            ]"#
        });
        let path = fixture.path("projects/project-a/chats/chat-a/chat-messages.json");
        std::fs::File::open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_767_312_000),
            ))
            .unwrap();
        let diagnostics = Diagnostics::new("codebuff");
        let entries = load_chat_file(&path, &diagnostics).unwrap();
        assert!(entries.is_empty());
        assert_eq!(diagnostics.skipped(SkipKind::InvalidTimestamp), 1);
    }

    #[test]
    fn loads_assistant_usage_from_chat_messages() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "projects/project-a/chats/2026-01-02T03-04-05.000Z/chat-messages.json": r#"[
                {"role":"user","text":"hello"},
                {"id":"assistant-message","role":"assistant","timestamp":"2026-01-02T03:04:06.000Z","metadata":{"model":"claude-sonnet-4-20250514","usage":{"inputTokens":100,"outputTokens":50,"cacheCreationInputTokens":20,"cacheReadInputTokens":10}},"credits":1.25}
            ]"#,
        });
        Environment::with_var(CODEBUFF_DATA_DIR_ENV, fixture.root()).run(|| {
            let pricing = ccusage_core::PricingMap::load_embedded();
            let shared = SharedArgs {
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                ..SharedArgs::default()
            };
            let entries = ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &shared,
                Default::default(),
                Some(&pricing),
            )
            .unwrap();

            let channel = fixture.root().file_name().unwrap().to_str().unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].date.to_string(), "2026-01-02");
            assert_eq!(
                entries[0].session_id.as_ref(),
                format!("{channel}/project-a/2026-01-02T03-04-05.000Z")
            );
            assert_eq!(entries[0].usage.input_tokens, 100);
            assert_eq!(entries[0].usage.output_tokens, 50);
            assert_eq!(entries[0].usage.cache_creation_input_tokens, 20);
            assert_eq!(entries[0].usage.cache_read_input_tokens, 10);
            assert_eq!(entries[0].credits, Some(1.25));
        })
    }

    #[test]
    fn malformed_chat_file_is_reported_and_an_unwritten_one_is_not() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "projects/project-a/chats/2026-01-02T03-04-05.000Z/chat-messages.json": r#"[
                {"id":"assistant-message","role":"assistant","timestamp":"2026-01-02T03:04:06.000Z","metadata":{"model":"claude-sonnet-4-20250514","usage":{"inputTokens":100,"outputTokens":50}}}
            ]"#,
            "projects/project-b/chats/2026-01-03T03-04-05.000Z/chat-messages.json": "[broken",
            "projects/project-c/chats/2026-01-04T03-04-05.000Z/chat-messages.json": "",
        });
        Environment::with_var(CODEBUFF_DATA_DIR_ENV, fixture.root()).run(|| {
            let shared = SharedArgs::default();
            let pricing = ccusage_core::PricingMap::load_embedded();
            let window = ReportWindow::from_shared(&shared);
            let diagnostics = Diagnostics::new("codebuff");
            let cx = LoadContext {
                shared: &shared,
                kind: AgentReportKind::Daily,
                window: &window,
                pricing: ccusage_adapter_common::source::ReportPricing::Supplied(&pricing),
                options: SourceOptions::default(),
                diagnostics: &diagnostics,
            };

            let entries = load_entries_with_context(&cx).unwrap();

            assert_eq!(entries.len(), 1);
            assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        })
    }

    #[test]
    fn focused_and_unified_reports_agree() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "projects/project-a/chats/2026-01-02T03-04-05.000Z/chat-messages.json": r#"[
                {"id":"assistant-message","role":"assistant","timestamp":"2026-01-02T03:04:06.000Z","metadata":{"model":"claude-sonnet-4-20250514","usage":{"inputTokens":100,"outputTokens":50}}}
            ]"#,
        });
        Environment::with_var(CODEBUFF_DATA_DIR_ENV, fixture.root()).run(|| {
            let shared = SharedArgs {
                offline: true,
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                ..SharedArgs::default()
            };

            assert_focused_matches_unified(
                &crate::SOURCE,
                ccusage_core::cli::AgentId::Codebuff,
                &shared,
            );
        })
    }

    #[test]
    fn falls_back_to_run_state_provider_usage() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "projects/project-a/chats/2026-01-02T03-04-05.000Z/chat-messages.json": r#"[
                {"variant":"agent","metadata":{"runState":{"sessionState":{"mainAgentState":{"messageHistory":[
                    {"role":"user","providerOptions":{}},
                    {"role":"assistant","providerOptions":{"codebuff":{"model":"openai/gpt-5","usage":{"prompt_tokens":100,"completion_tokens":50,"prompt_tokens_details":{"cached_tokens":10}}}}}
                ]}}}}}
            ]"#,
        });
        Environment::with_var(CODEBUFF_DATA_DIR_ENV, fixture.root()).run(|| {
            let pricing = ccusage_core::PricingMap::load_embedded();
            let entries = ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &SharedArgs::default(),
                Default::default(),
                Some(&pricing),
            )
            .unwrap();

            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].model.as_deref(), Some("openai/gpt-5"));
            assert_eq!(entries[0].usage.input_tokens, 100);
            assert_eq!(entries[0].usage.output_tokens, 50);
            assert_eq!(entries[0].usage.cache_read_input_tokens, 10);
        })
    }

    #[test]
    fn prices_recorded_provider_output_when_primary_usage_only_has_a_total() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "projects/p/chats/2026-01-02T03-04-05.000Z/chat-messages.json": r#"[
                {"role":"assistant","metadata":{"runState":{"sessionState":{"mainAgentState":{"messageHistory":[
                    {"role":"assistant","providerOptions":{
                        "usage":{"totalTokens":180},
                        "codebuff":{"model":"sample","usage":{"inputTokens":100,"outputTokens":50}}
                    }}
                ]}}}}}
            ]"#,
        });
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(r#"{"sample":{"input_cost_per_token":1,"output_cost_per_token":2}}"#);
        Environment::with_var(CODEBUFF_DATA_DIR_ENV, fixture.root()).run(|| {
            let entries = ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &SharedArgs::default(),
                Default::default(),
                Some(&pricing),
            )
            .unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].cost, 200.0);
            assert_eq!(entries[0].usage.output_tokens, 50);
            assert_eq!(entries[0].extra_total_tokens, 30);
        });
    }

    #[test]
    fn falls_back_to_total_tokens_when_codebuff_parts_are_missing() {
        let fixture = fs_fixture!({
            "projects/p/chats/2026-01-02T03-04-05.000Z/chat-messages.json": r#"[
                {"role":"assistant","metadata":{"usage":{"totalTokens":789}}},
                {"role":"assistant","metadata":{"usage":{"totalTokens":789,"outputTokens":0}}}
            ]"#,
        });
        let mut entries = super::super::parser::load_chat_file(
            &fixture.path("projects/p/chats/2026-01-02T03-04-05.000Z/chat-messages.json"),
            &Diagnostics::new("codebuff"),
        )
        .unwrap()
        .into_iter()
        .map(|entry| to_loaded_entry(entry, &JiffTimeZone::UTC, PriceTable::Display));
        let missing_output = entries.next().unwrap();
        assert_eq!(missing_output.usage.output_tokens, 789);
        assert_eq!(missing_output.extra_total_tokens, 0);
        let explicit_zero = entries.next().unwrap();
        assert_eq!(explicit_zero.usage.output_tokens, 0);
        assert_eq!(explicit_zero.extra_total_tokens, 789);
    }

    #[test]
    fn report_includes_credits() {
        let entry = LoadedEntry {
            usage: TokenUsageRaw {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_input_tokens: 20,
                cache_read_input_tokens: 10,
                speed: None,
                cache_creation: None,
            },
            message_id: Some("message-a".to_string()),
            version: None,
            recorded_cost: None,
            timestamp: parse_ts_timestamp("2026-01-02T03:04:06.000Z").unwrap(),
            date: ccusage_core::cli::IsoDate::parse("2026-01-02").unwrap(),
            project: Arc::from("codebuff"),
            session_id: Arc::from("session-a"),
            project_path: Arc::from("Codebuff"),
            cost: 0.02,
            extra_total_tokens: 0,
            credits: Some(1.25),
            model: Some("claude-sonnet-4-20250514".to_string()),
            usage_limit_reset_time: None,
            missing_pricing_model: None,
            message_count: None,
            stored_cost: None,
            estimated_pricing: false,
        };
        let rows = ccusage_adapter_common::source::EntrySource::summarize(
            &crate::SOURCE,
            &[entry],
            ccusage_adapter_common::source::SummaryKind::for_report(
                AgentReportKind::Daily,
                ccusage_core::cli::WeekDay::Monday,
            ),
        )
        .unwrap();
        let report = ccusage_adapter_common::source::EntrySource::report_json(
            &crate::SOURCE,
            &rows,
            AgentReportKind::Daily,
        );

        assert_eq!(report["daily"][0]["credits"], serde_json::json!(1.25));
        assert_eq!(report["totals"]["credits"], serde_json::json!(1.25));
    }
}
