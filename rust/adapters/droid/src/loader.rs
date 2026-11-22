use ccusage_core::cost::PriceTable;
use std::sync::Arc;

use jiff::tz::TimeZone as JiffTimeZone;

use super::{
    parser::{DroidEntry, load_settings_file, price_droid},
    paths::discover_settings_files,
};
use ccusage_adapter_common::{
    dedupe::dedupe_last_wins, source::LoadContext, units::read_units_flat,
};

use crate::{LoadedEntry, Result, date_at};

pub(crate) fn load_entries_with_context(cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
    let pricing = cx.pricing.table(cx.shared.mode);
    let tz = cx.window.timezone();
    let mut files = discover_settings_files(cx.diagnostics)?;
    files.sort();
    let mut parsed = read_units_flat(
        &files,
        cx.shared.single_thread,
        cx.diagnostics,
        load_settings_file,
    );
    parsed.sort_by_key(|entry| entry.timestamp);
    Ok(dedupe_last_wins(parsed, |entry| entry.session_id.clone())
        .into_iter()
        .rev()
        .map(|entry| to_loaded_entry(entry, tz, pricing))
        .collect())
}

fn to_loaded_entry(entry: DroidEntry, tz: &JiffTimeZone, pricing: PriceTable<'_>) -> LoadedEntry {
    let (cost, missing_pricing_model, estimated_pricing) = price_droid(&entry, pricing);
    LoadedEntry {
        date: date_at(entry.timestamp, tz),
        timestamp: entry.timestamp,
        project: Arc::from("droid"),
        session_id: Arc::from(entry.session_id.as_str()),
        project_path: Arc::from("Droid"),
        cost,
        credits: None,
        extra_total_tokens: entry.additional.total(),
        model: Some(entry.model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        estimated_pricing,
        message_count: None,
        usage: entry.usage,
        message_id: Some(format!("droid:{}", entry.session_id)),
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
        let fixture = fs_fixture!({"session.settings.json": r#"{"model":"sample","providerLockTimestamp":"2026-05-01T01:02:03Z","tokenUsage":{"inputTokens":100,"outputTokens":50}}"#});
        Environment::with_var(DROID_SESSIONS_DIR_ENV, fixture.root()).run(|| {
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

    use crate::cli::SharedArgs;
    use ccusage_adapter_common::parity::assert_focused_matches_unified;
    use ccusage_adapter_common::source::{LoadContext, SourceOptions};
    use ccusage_core::{Diagnostics, ReportWindow, SkipKind};
    use ccusage_test_support::{Environment, fs_fixture};
    use serde_json::json;

    use super::super::{
        parser::{normalize_droid_model_name, parse_token_usage},
        paths::DROID_SESSIONS_DIR_ENV,
    };
    use super::*;
    use crate::{TokenUsageRaw, cli::AgentReportKind, parse_ts_timestamp};
    #[test]
    fn normalizes_droid_model_names() {
        assert_eq!(
            normalize_droid_model_name("custom:Claude-Opus-4.5-Thinking-[Anthropic]-0"),
            "claude-opus-4-5-thinking-0"
        );
        assert_eq!(
            normalize_droid_model_name("Claude-Sonnet-4-[Anthropic]"),
            "claude-sonnet-4"
        );
        assert_eq!(
            normalize_droid_model_name("gemini-2.5-pro"),
            "gemini-2-5-pro"
        );
    }

    #[test]
    fn falls_back_to_total_tokens_when_droid_parts_are_missing() {
        let usage = parse_token_usage(Some(&serde_json::json!({
            "totalTokens": 456
        })))
        .unwrap()
        .unwrap();

        let extra_total_tokens = usage.additional.total();
        let usage = usage.usage;
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(extra_total_tokens, 456);
    }

    #[test]
    fn loads_usage_from_droid_settings_files() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "session-a.settings.json": r#"{
                "model": "Claude-Sonnet-4-[Anthropic]",
                "providerLock": "anthropic",
                "providerLockTimestamp": "2026-05-01T01:02:03.000Z",
                "tokenUsage": {
                    "inputTokens": 100,
                    "outputTokens": 50,
                    "cacheCreationTokens": 20,
                    "cacheReadTokens": 10,
                    "thinkingTokens": 5
                }
            }"#,
            "zero.settings.json": r#"{"model":"gpt-5","tokenUsage":{"inputTokens":0}}"#,
        });
        Environment::with_var(DROID_SESSIONS_DIR_ENV, fixture.root()).run(|| {
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

            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].date.to_string(), "2026-05-01");
            assert_eq!(entries[0].session_id.as_ref(), "session-a");
            assert_eq!(entries[0].model.as_deref(), Some("claude-sonnet-4"));
            assert_eq!(entries[0].usage.input_tokens, 100);
            assert_eq!(entries[0].usage.output_tokens, 50);
            assert_eq!(entries[0].usage.cache_creation_input_tokens, 20);
            assert_eq!(entries[0].usage.cache_read_input_tokens, 10);
            assert_eq!(entries[0].extra_total_tokens, 5);
        })
    }

    #[test]
    fn malformed_settings_file_is_reported_and_an_unwritten_one_is_not() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "session-a.settings.json": r#"{
                "model": "Claude-Sonnet-4-[Anthropic]",
                "providerLock": "anthropic",
                "providerLockTimestamp": "2026-05-01T01:02:03.000Z",
                "tokenUsage": {"inputTokens": 100, "outputTokens": 50}
            }"#,
            "session-b.settings.json": "{broken",
            "session-c.settings.json": "",
        });
        Environment::with_var(DROID_SESSIONS_DIR_ENV, fixture.root()).run(|| {
            let shared = SharedArgs::default();
            let pricing = ccusage_core::PricingMap::load_embedded();
            let window = ReportWindow::from_shared(&shared);
            let diagnostics = Diagnostics::new("droid");
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
    fn overflowing_token_counts_are_diagnosed_instead_of_panicking() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "overflow.settings.json": serde_json::json!({
                "model": "gpt-5",
                "providerLockTimestamp": "2026-05-01T01:02:03.000Z",
                "tokenUsage": {"inputTokens": u64::MAX, "outputTokens": 1}
            })
            .to_string(),
            "thinking-overflow.settings.json": serde_json::json!({
                "model": "gpt-5",
                "providerLockTimestamp": "2026-05-01T01:02:03.000Z",
                "tokenUsage": {"inputTokens": u64::MAX, "thinkingTokens": 1}
            })
            .to_string(),
            "healthy.settings.json": r#"{
                "model": "gpt-5",
                "providerLockTimestamp": "2026-05-02T01:02:03.000Z",
                "tokenUsage": {"inputTokens": 100, "outputTokens": 50}
            }"#,
        });
        Environment::with_var(DROID_SESSIONS_DIR_ENV, fixture.root()).run(|| {
            let shared = SharedArgs::default();
            let pricing = ccusage_core::PricingMap::load_embedded();
            let window = ReportWindow::from_shared(&shared);
            let diagnostics = Diagnostics::new("droid");
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
            assert_eq!(entries[0].session_id.as_ref(), "healthy");
            assert_eq!(entries[0].usage.input_tokens, 100);
            assert_eq!(entries[0].usage.output_tokens, 50);
            assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 2);
        })
    }

    #[test]
    fn focused_and_unified_reports_agree() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "session-a.settings.json": r#"{
                "model": "Claude-Sonnet-4-[Anthropic]",
                "providerLock": "anthropic",
                "providerLockTimestamp": "2026-05-01T01:02:03.000Z",
                "tokenUsage": {"inputTokens": 100, "outputTokens": 50}
            }"#,
        });
        Environment::with_var(DROID_SESSIONS_DIR_ENV, fixture.root()).run(|| {
            let shared = SharedArgs {
                offline: true,
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                ..SharedArgs::default()
            };

            assert_focused_matches_unified(
                &crate::SOURCE,
                ccusage_core::cli::AgentId::Droid,
                &shared,
            );
        })
    }

    #[test]
    fn falls_back_to_sidecar_jsonl_model() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "session-b.settings.json": r#"{
                "providerLock": "anthropic",
                "providerLockTimestamp": "2026-05-02T01:02:03.000Z",
                "tokenUsage": {"inputTokens": 10, "outputTokens": 20}
            }"#,
            "session-b.jsonl": r#"{"content":"Model: Claude Opus 4.5 Thinking [Anthropic]"}"#,
        });
        Environment::with_var(DROID_SESSIONS_DIR_ENV, fixture.root()).run(|| {
            let pricing = ccusage_core::PricingMap::load_embedded();
            let entries = ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &SharedArgs::default(),
                Default::default(),
                Some(&pricing),
            )
            .unwrap();

            assert_eq!(entries.len(), 1);
            assert_eq!(
                entries[0].model.as_deref(),
                Some("claude-opus-4-5-thinking")
            );
        })
    }

    #[test]
    fn keeps_latest_snapshot_for_duplicate_session_ids() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "archive/session-c.settings.json": r#"{
                "model": "gpt-5",
                "providerLock": "openai",
                "providerLockTimestamp": "2026-05-01T01:02:03.000Z",
                "tokenUsage": {"inputTokens": 10, "outputTokens": 20}
            }"#,
            "session-c.settings.json": r#"{
                "model": "gpt-5",
                "providerLock": "openai",
                "providerLockTimestamp": "2026-05-02T01:02:03.000Z",
                "tokenUsage": {"inputTokens": 100, "outputTokens": 200}
            }"#,
        });
        Environment::with_var(DROID_SESSIONS_DIR_ENV, fixture.root()).run(|| {
            let pricing = ccusage_core::PricingMap::load_embedded();
            let entries = ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &SharedArgs::default(),
                Default::default(),
                Some(&pricing),
            )
            .unwrap();

            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].session_id.as_ref(), "session-c");
            assert_eq!(entries[0].usage.input_tokens, 100);
            assert_eq!(entries[0].usage.output_tokens, 200);
        })
    }

    #[test]
    fn report_total_includes_thinking_tokens() {
        let entry = LoadedEntry {
            usage: TokenUsageRaw {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_input_tokens: 20,
                cache_read_input_tokens: 10,
                speed: None,
                cache_creation: None,
            },
            message_id: Some("droid:session-a".to_string()),
            version: None,
            recorded_cost: None,
            timestamp: parse_ts_timestamp("2026-05-01T01:02:03.000Z").unwrap(),
            date: ccusage_core::cli::IsoDate::parse("2026-05-01").unwrap(),
            project: Arc::from("droid"),
            session_id: Arc::from("session-a"),
            project_path: Arc::from("Droid"),
            cost: 0.0,
            credits: None,
            extra_total_tokens: 5,
            model: Some("claude-sonnet-4".to_string()),
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

        assert_eq!(report["daily"][0]["totalTokens"], json!(185));
        assert_eq!(report["totals"]["totalTokens"], json!(185));
    }
}
