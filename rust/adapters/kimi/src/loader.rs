use ccusage_adapter_common::{
    dedupe::dedupe_preferred, source::LoadContext, units::read_typed_units_flat,
};

use crate::{LoadedEntry, Result};

use super::{
    parser::{KimiUsageEntry, kimi_entry_to_loaded, read_wire_file},
    paths::discover_wire_files,
};

pub(crate) fn load_entries(cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
    let mut files = discover_wire_files(cx.diagnostics)?;
    let stale = cx.stale_files();
    files.retain(|file| !stale(file.as_ref()));
    let pricing = cx.pricing.table(cx.shared.mode);
    let loaded = read_typed_units_flat(&files, cx.shared.single_thread, cx.diagnostics, |file| {
        read_wire_file(file, cx.diagnostics)
    });
    let mut entries = dedupe_preferred(loaded, KimiUsageEntry::dedupe_key, |entry, other| {
        entry.billable_output_tokens() > other.billable_output_tokens()
    })
    .into_iter()
    .map(|entry| kimi_entry_to_loaded(entry, cx.window.timezone(), pricing))
    .collect::<Vec<_>>();
    LoadedEntry::sort_by_timestamp(&mut entries);
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::super::paths::KIMI_DATA_DIR_ENV;
    use ccusage_core::{Diagnostics, PricingMap, SkipKind, cli::SharedArgs};
    use ccusage_test_support::{Environment, fs_fixture};

    #[test]
    fn unexplained_kimi_tokens_count_only_towards_total_usage() {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::default();
        pricing.load_json(r#"{"sample":{"input_cost_per_token":1,"output_cost_per_token":2}}"#);
        let fixture = fs_fixture!({
            "config.json": r#"{"model":"sample"}"#,
            "sessions/group/session-a/wire.jsonl":
                r#"{"timestamp":1770983427,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":100,"total":150}}}}"#,
        });
        Environment::with_var(KIMI_DATA_DIR_ENV, fixture.root()).run(|| {
            let entries = ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &SharedArgs::default(),
                Default::default(),
                Some(&pricing),
            )
            .unwrap();
            assert_eq!(entries.len(), 1);
            let entry = &entries[0];
            assert_eq!(entry.usage.input_tokens, 100);
            assert_eq!(entry.cost, 100.0);
            assert_eq!(entry.usage.output_tokens, 0);
            assert_eq!(entry.extra_total_tokens, 50);
            assert_eq!(
                ccusage_core::checked_total_usage_tokens(entry.usage, entry.extra_total_tokens),
                Some(150)
            );
        });
    }

    #[test]
    fn duplicate_kimi_reports_prefer_the_copy_with_recorded_output() {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::default();
        pricing.load_json(r#"{"sample":{"input_cost_per_token":1,"output_cost_per_token":2}}"#);
        let usage = |tokens: &str| {
            format!(
                "{{\"timestamp\":1770983427,\"message\":{{\"type\":\"StatusUpdate\",\"payload\":{{\"message_id\":\"duplicate\",\"token_usage\":{tokens}}}}}}}"
            )
        };
        let partial = usage(r#"{"input_other":100,"total":150}"#);
        let recorded = usage(r#"{"input_other":100,"output":50,"total":150}"#);
        for rows in [[&partial, &recorded], [&recorded, &partial]] {
            let fixture = fs_fixture!({
                "config.json": r#"{"model":"sample"}"#,
                "sessions/group/session-a/wire.jsonl": rows.map(String::as_str).join("\n"),
            });
            Environment::with_var(KIMI_DATA_DIR_ENV, fixture.root()).run(|| {
                let entries = ccusage_adapter_common::testing::entries(
                    &crate::SOURCE,
                    &SharedArgs::default(),
                    Default::default(),
                    Some(&pricing),
                )
                .unwrap();
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].cost, 200.0);
            })
        }
    }

    #[test]
    fn loads_status_update_token_usage_from_wire_files() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "config.json": r#"{"model":"kimi-k2"}"#,
            "sessions/group/session-a/wire.jsonl": [
                r#"{"type":"metadata","protocol_version":"1.3"}"#,
                r#"{"timestamp":1770983426.420942,"message":{"type":"TurnBegin","payload":{"user_input":"hello"}}}"#,
                r#"{"timestamp":1770983427.123,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":100,"output":50,"input_cache_read":10,"input_cache_creation":20},"message_id":"msg-1"}}}"#,
            ]
            .join("\n"),
        });
        Environment::with_var(KIMI_DATA_DIR_ENV, fixture.root()).run(|| {
            let shared = SharedArgs {
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                ..SharedArgs::default()
            };
            let entries = ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &shared,
                Default::default(),
                Some(&PricingMap::load_embedded()),
            )
            .unwrap();

            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].date.to_string(), "2026-02-13");
            assert_eq!(entries[0].session_id.as_ref(), "session-a");
            assert_eq!(entries[0].model.as_deref(), Some("kimi-k2"));
            assert_eq!(entries[0].usage.input_tokens, 100);
            assert_eq!(entries[0].usage.output_tokens, 50);
            assert_eq!(entries[0].usage.cache_creation_input_tokens, 20);
            assert_eq!(entries[0].usage.cache_read_input_tokens, 10);
        })
    }

    #[test]
    fn loads_kimi_code_usage_record_format() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "sessions/workspace/session-b/agents/agent-1/wire.jsonl": [
                r#"{"type":"usage.record","model":"kimi-code/kimi-for-coding","usage":{"inputOther":3064,"output":76,"inputCacheRead":14848,"inputCacheCreation":0},"usageScope":"turn","time":1782113184943}"#,
                r#"{"type":"usage.record","model":"kimi-code/kimi-for-coding","usage":{"inputOther":5000,"output":200,"inputCacheRead":20000,"inputCacheCreation":100},"usageScope":"session","time":1782113185000}"#,
            ]
            .join("\n"),
        });
        Environment::with_var(KIMI_DATA_DIR_ENV, fixture.root()).run(|| {
            let shared = SharedArgs {
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                ..SharedArgs::default()
            };
            let entries = ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &shared,
                Default::default(),
                Some(&PricingMap::load_embedded()),
            )
            .unwrap();

            assert_eq!(entries.len(), 1, "session-scoped record must be skipped");
            assert_eq!(entries[0].session_id.as_ref(), "session-b");
            assert_eq!(entries[0].model.as_deref(), Some("kimi-for-coding"));
            assert_eq!(entries[0].usage.input_tokens, 3064);
            assert_eq!(entries[0].usage.output_tokens, 76);
            assert_eq!(entries[0].usage.cache_read_input_tokens, 14848);
            assert_eq!(entries[0].usage.cache_creation_input_tokens, 0);
        })
    }

    #[test]
    fn skips_malformed_and_zero_token_wire_lines() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "sessions/group/session-a/wire.jsonl": [
                "not json",
                r#"{"timestamp":1770983427,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":0,"output":0,"input_cache_read":0,"input_cache_creation":0}}}}"#,
            ]
            .join("\n"),
        });
        Environment::with_var(KIMI_DATA_DIR_ENV, fixture.root()).run(|| {
            let entries = ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &SharedArgs::default(),
                Default::default(),
                Some(&PricingMap::load_embedded()),
            )
            .unwrap();

            assert!(entries.is_empty());
        })
    }

    #[test]
    fn skips_a_wire_file_last_changed_before_the_window() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "sessions/group/stale/wire.jsonl":
                r#"{"timestamp":1770983427,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":100,"output":50}}}}"#,
            "sessions/group/live/wire.jsonl":
                r#"{"timestamp":1770983427,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":7,"output":3}}}}"#,
        });
        for (session, modified) in [
            ("stale", "2099-01-01T00:00:00Z"),
            ("live", "2099-01-05T00:00:00Z"),
        ] {
            ccusage_test_support::set_modified(
                &fixture.path(format!("sessions/group/{session}/wire.jsonl")),
                ccusage_test_support::instant(modified),
            );
        }
        Environment::with_var(KIMI_DATA_DIR_ENV, fixture.root()).run(|| {
            let shared = SharedArgs {
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                since: Some("20990104".parse().unwrap()),
                clock: ccusage_core::cli::ReportClock::Fixed(
                    ccusage_test_support::instant("2099-01-06T00:00:00Z").into(),
                ),
                ..SharedArgs::default()
            };
            let entries = ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &shared,
                Default::default(),
                Some(&PricingMap::load_embedded()),
            )
            .unwrap();

            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].session_id.as_ref(), "live");
            assert_eq!(entries[0].usage.input_tokens, 7);
        })
    }

    #[test]
    fn diagnoses_malformed_usage_record_and_keeps_later_wire_usage() {
        let fixture = fs_fixture!({
            "sessions/group/session-a/wire.jsonl": [
                r#"{"type":"usage.record","model":"kimi-for-coding","usage":"SECRET_CANARY","usageScope":"turn"}"#,
                r#"{"type":"usage.record","model":"kimi-for-coding","usage":{"inputOther":100,"output":20},"usageScope":"turn","time":1770983427000}"#,
            ]
            .join("\n"),
        });
        let diagnostics = Diagnostics::new("kimi");

        let entries = super::super::parser::read_wire_file(
            &crate::paths::WireFile::from_paths(vec![
                fixture.path("sessions/group/session-a/wire.jsonl"),
            ])
            .remove(0),
            &diagnostics,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        let warnings = diagnostics
            .warnings(&SharedArgs {
                debug: true,
                ..SharedArgs::default()
            })
            .join("\n");
        assert!(!warnings.contains("SECRET_CANARY"));
    }
}
