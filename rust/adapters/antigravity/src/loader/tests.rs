use std::ffi::OsString;

use ccusage_test_support::{Environment, Fixture};
use serde_json::json;

use std::path::Path;

use ccusage_adapter_common::source::SourceOptions;
use ccusage_core::{Diagnostics, SkipKind};

use super::*;
use crate::cli::SharedArgs;
use crate::parser::test_support::{
    UsageFixture, create_database, metadata_blob, metadata_blob_with_steps, step_metadata_blob,
    trajectory_metadata_blob,
};
use ccusage_core::PricingMap;

fn shared(single_thread: bool) -> SharedArgs {
    SharedArgs {
        mode: crate::cli::CostMode::Calculate,
        single_thread,
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        ..SharedArgs::default()
    }
}

fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    load_with_diagnostics(shared, pricing).map(|(entries, _)| entries)
}

fn load_with_diagnostics(
    shared: &SharedArgs,
    pricing: &PricingMap,
) -> Result<(Vec<LoadedEntry>, Diagnostics)> {
    ccusage_adapter_common::testing::load_entries(
        &crate::SOURCE,
        shared,
        SourceOptions::default(),
        Some(pricing),
    )
}

fn skipped_databases() -> Diagnostics {
    let (entries, diagnostics) =
        load_with_diagnostics(&shared(true), &PricingMap::load_embedded()).unwrap();
    assert!(entries.is_empty());
    diagnostics
}

fn assert_skipped(diagnostics: &Diagnostics, kind: SkipKind, label: &str, path: &Path) {
    assert_eq!(diagnostics.skipped(kind), 1, "{kind:?}");
    let lines = diagnostics.warnings(&SharedArgs {
        debug: true,
        ..shared(true)
    });
    let prefix = format!("antigravity: {label}: {}: ", path.display());
    assert!(
        lines.iter().any(|line| line.starts_with(&prefix)),
        "{lines:?}"
    );
}

fn load_from_fixture(fixture: &Fixture, single_thread: bool) -> Vec<LoadedEntry> {
    let pricing = PricingMap::load_embedded();
    load_from_fixture_with_pricing(fixture, single_thread, &pricing)
}

fn load_from_fixture_with_pricing(
    fixture: &Fixture,
    single_thread: bool,
    pricing: &PricingMap,
) -> Vec<LoadedEntry> {
    Environment::with_vars([(
        super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
        Some(OsString::from(fixture.root())),
    )])
    .run(|| load_entries(&shared(single_thread), pricing).unwrap())
}

fn snapshot_pricing() -> PricingMap {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = PricingMap::default();
    assert_eq!(
        pricing.load_json(
            r#"{
                    "gemini-3-pro": {
                        "input_cost_per_token": 0.000001,
                        "output_cost_per_token": 0.000002,
                        "cache_creation_input_token_cost": 0.000001,
                        "cache_read_input_token_cost": 0.0000001
                    },
                    "gemini-3.6-flash": {
                        "input_cost_per_token": 0.000003,
                        "output_cost_per_token": 0.000004
                    },
                    "claude-opus-5-5": {
                        "input_cost_per_token": 0.000005,
                        "output_cost_per_token": 0.000025
                    }
                }"#
        ),
        3
    );
    pricing
}

#[test]
fn loads_real_schema_rows_with_continuations_and_token_buckets() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = Fixture::new();
    let db_path = fixture.path("conversations/session.db");
    let first_usage = UsageFixture {
        input_tokens: 400,
        total_output_tokens: 200,
        cache_creation_tokens: 30,
        cache_read_tokens: 500,
        reasoning_tokens: 50,
        visible_output_tokens: 150,
        message_id: Some("message-1"),
        response_id: Some("response-1"),
        provider_assigned_message_id: Some("provider-1"),
        ..UsageFixture::default()
    };
    let continuation_usage = UsageFixture {
        input_tokens: 300,
        total_output_tokens: 100,
        reasoning_tokens: 25,
        visible_output_tokens: 75,
        message_id: Some("message-2"),
        response_id: Some("response-2"),
        ..UsageFixture::default()
    };
    create_database(
        &db_path,
        &[
            (
                2,
                metadata_blob(
                    None,
                    Some(continuation_usage),
                    Some((1_778_000_001, 0)),
                    &[],
                ),
            ),
            (
                1,
                metadata_blob(
                    Some("Gemini 3 Pro"),
                    Some(first_usage),
                    Some((1_778_000_000, 123_000_000)),
                    &[],
                ),
            ),
        ],
        &[],
        &[],
    );

    let entries = load_from_fixture(&fixture, true);

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].model.as_deref(), Some("gemini-3-pro"));
    assert_eq!(entries[1].model.as_deref(), Some("gemini-3-pro"));
    assert_eq!(entries[0].message_id.as_deref(), Some("response-1"));
    assert_eq!(entries[0].usage.input_tokens, 400);
    assert_eq!(entries[0].usage.cache_creation_input_tokens, 30);
    assert_eq!(entries[0].usage.cache_read_input_tokens, 500);
    assert_eq!(entries[0].usage.output_tokens, 150);
    assert_eq!(entries[0].extra_total_tokens, 50);
    assert_eq!(entries[1].usage.input_tokens, 300);
    assert_eq!(entries[1].usage.output_tokens, 75);
    assert_eq!(entries[1].extra_total_tokens, 25);
    assert!(entries[0].cost > 0.0);
}

#[test]
fn decodes_model_id_without_counting_it_as_input_tokens() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = Fixture::new();
    create_database(
        &fixture.path("conversations/model-id.db"),
        &[(
            1,
            metadata_blob(
                None,
                Some(UsageFixture {
                    model_id: Some(246),
                    input_tokens: 4,
                    total_output_tokens: 6,
                    visible_output_tokens: 6,
                    response_id: Some("model-id-response"),
                    ..UsageFixture::default()
                }),
                Some((1_778_000_000, 0)),
                &[],
            ),
        )],
        &[],
        &[],
    );

    let entries = load_from_fixture(&fixture, true);

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].model.as_deref(), Some("gemini-2.5-pro"));
    assert_eq!(entries[0].usage.input_tokens, 4);
}

#[test]
fn loads_multiple_databases_consistently() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = Fixture::new();
    create_database(
        &fixture.path("one/conversations/one.db"),
        &[(
            1,
            metadata_blob(
                Some("gemini-3-pro"),
                Some(UsageFixture {
                    input_tokens: 20,
                    total_output_tokens: 45,
                    visible_output_tokens: 40,
                    reasoning_tokens: 5,
                    response_id: Some("one"),
                    ..UsageFixture::default()
                }),
                Some((1_778_000_000, 0)),
                &[],
            ),
        )],
        &[],
        &[],
    );
    create_database(
        &fixture.path("two/conversations/two.db"),
        &[(
            1,
            metadata_blob(
                Some("gemini-3-pro"),
                Some(UsageFixture {
                    input_tokens: 21,
                    total_output_tokens: 47,
                    visible_output_tokens: 41,
                    reasoning_tokens: 6,
                    response_id: Some("two"),
                    ..UsageFixture::default()
                }),
                Some((1_778_000_002, 0)),
                &[],
            ),
        )],
        &[],
        &[],
    );
    let override_value = format!(
        "{},{}",
        fixture.path("one").display(),
        fixture.path("two").display()
    );
    Environment::with_vars([(
        super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
        Some(OsString::from(override_value)),
    )])
    .run(|| {
        let single_threaded = load_entries(&shared(true), &PricingMap::load_embedded()).unwrap();
        let default_mode = load_entries(&shared(false), &PricingMap::load_embedded()).unwrap();

        assert_eq!(
            single_threaded
                .iter()
                .map(|entry| entry.message_id.clone())
                .collect::<Vec<_>>(),
            default_mode
                .iter()
                .map(|entry| entry.message_id.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(single_threaded.len(), 2);
    })
}

#[test]
fn deduplicates_response_ids_across_databases() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = Fixture::new();
    create_database(
        &fixture.path("first/conversations/first.db"),
        &[(
            1,
            metadata_blob(
                Some("gemini-3-pro"),
                Some(UsageFixture {
                    input_tokens: 20,
                    total_output_tokens: 40,
                    visible_output_tokens: 35,
                    reasoning_tokens: 5,
                    message_id: Some("shared-message"),
                    response_id: Some("shared-response"),
                    provider_assigned_message_id: Some("shared-provider"),
                    ..UsageFixture::default()
                }),
                Some((1_778_000_000, 0)),
                &[],
            ),
        )],
        &[],
        &[],
    );
    create_database(
        &fixture.path("second/conversations/second.db"),
        &[(
            1,
            metadata_blob(
                Some("gemini-3-pro"),
                Some(UsageFixture {
                    input_tokens: 200,
                    total_output_tokens: 400,
                    visible_output_tokens: 350,
                    reasoning_tokens: 50,
                    message_id: Some("shared-message"),
                    response_id: Some("retry-response"),
                    provider_assigned_message_id: Some("shared-provider"),
                    ..UsageFixture::default()
                }),
                Some((1_778_000_001, 0)),
                &[],
            ),
        )],
        &[],
        &[],
    );
    let override_value = format!(
        "{},{}",
        fixture.path("first").display(),
        fixture.path("second").display()
    );
    Environment::with_vars([(
        super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
        Some(OsString::from(override_value)),
    )])
    .run(|| {
        let entries = load_entries(&shared(true), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message_id.as_deref(), Some("shared-response"));
        assert_eq!(entries[0].usage.input_tokens, 200);
        assert_eq!(entries[0].usage.output_tokens, 350);
    })
}

#[test]
fn collects_steps_and_retries_and_deduplicates_source_copies() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = Fixture::new();
    let shared_usage = UsageFixture {
        input_tokens: 100,
        total_output_tokens: 50,
        cache_creation_tokens: 5,
        cache_read_tokens: 7,
        reasoning_tokens: 10,
        visible_output_tokens: 40,
        message_id: Some("message-1"),
        response_id: Some("response-1"),
        provider_assigned_message_id: Some("provider-1"),
        ..UsageFixture::default()
    };
    let retry_usage = UsageFixture {
        input_tokens: 80,
        total_output_tokens: 30,
        reasoning_tokens: 5,
        visible_output_tokens: 25,
        message_id: Some("message-1"),
        response_id: Some("retry-response"),
        provider_assigned_message_id: Some("provider-1"),
        ..UsageFixture::default()
    };
    let step_retry_usage = UsageFixture {
        input_tokens: 11,
        total_output_tokens: 22,
        reasoning_tokens: 2,
        visible_output_tokens: 20,
        message_id: Some("step-retry-message"),
        response_id: Some("step-retry-response"),
        provider_assigned_message_id: Some("step-retry-provider"),
        ..UsageFixture::default()
    };
    create_database(
        &fixture.path("conversations/session.db"),
        &[(
            1,
            metadata_blob(
                Some("gemini-3-pro"),
                Some(shared_usage),
                Some((1_778_100_001, 0)),
                &[retry_usage],
            ),
        )],
        &[
            (
                1,
                step_metadata_blob(
                    None,
                    Some(shared_usage),
                    Some((1_778_100_000, 0)),
                    &[],
                    Some(7),
                ),
            ),
            (
                2,
                step_metadata_blob(
                    Some("gemini-3-flash-agent"),
                    None,
                    Some((1_778_100_002, 0)),
                    &[step_retry_usage],
                    Some(7),
                ),
            ),
        ],
        &[],
    );

    let entries = load_from_fixture(&fixture, true);

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].usage.input_tokens, 100);
    assert_eq!(entries[0].usage.cache_creation_input_tokens, 5);
    assert_eq!(entries[0].usage.cache_read_input_tokens, 7);
    assert_eq!(entries[0].usage.output_tokens, 40);
    assert_eq!(entries[0].extra_total_tokens, 10);
    assert_eq!(entries[0].model.as_deref(), Some("gemini-3-pro"));
    assert_eq!(entries[1].model.as_deref(), Some("gemini-3.5-flash-high"));
    assert_eq!(entries[1].usage.input_tokens, 11);
}

#[test]
fn deduplicates_unkeyed_mirror_copies_via_step_index() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = Fixture::new();
    let unkeyed = UsageFixture {
        input_tokens: 15_027,
        total_output_tokens: 49,
        visible_output_tokens: 49,
        cache_read_tokens: 500_131,
        ..UsageFixture::default()
    };
    create_database(
        &fixture.path("conversations/session.db"),
        &[(
            0,
            metadata_blob_with_steps(
                Some("gemini-4-argon"),
                Some(unkeyed),
                Some((1_778_100_001, 0)),
                &[unkeyed],
                &[1],
            ),
        )],
        &[(
            1,
            step_metadata_blob(
                Some("gemini-4-argon-medium"),
                Some(unkeyed),
                Some((1_778_100_000, 0)),
                &[],
                None,
            ),
        )],
        &[],
    );

    let entries = load_from_fixture(&fixture, true);

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].usage.input_tokens, 15_027);
    assert_eq!(entries[0].usage.cache_read_input_tokens, 500_131);
    assert_eq!(entries[0].usage.output_tokens, 49);
}

#[test]
fn drops_unkeyed_retry_mirror_without_step_link() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = Fixture::new();
    let unkeyed = UsageFixture {
        input_tokens: 100,
        total_output_tokens: 50,
        visible_output_tokens: 50,
        ..UsageFixture::default()
    };
    create_database(
        &fixture.path("conversations/session.db"),
        &[(
            1,
            metadata_blob(
                Some("gemini-3-pro"),
                Some(unkeyed),
                Some((1_778_100_001, 0)),
                &[unkeyed],
            ),
        )],
        &[],
        &[],
    );

    let entries = load_from_fixture(&fixture, true);

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].usage.input_tokens, 100);
    assert_eq!(entries[0].usage.output_tokens, 50);
}

#[test]
fn keeps_unkeyed_failed_attempt_when_retries_mirror_primary() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = Fixture::new();
    let success = UsageFixture {
        input_tokens: 100,
        total_output_tokens: 50,
        visible_output_tokens: 50,
        ..UsageFixture::default()
    };
    let failed = UsageFixture {
        input_tokens: 100,
        total_output_tokens: 10,
        visible_output_tokens: 10,
        ..UsageFixture::default()
    };
    create_database(
        &fixture.path("conversations/session.db"),
        &[(
            0,
            metadata_blob_with_steps(
                Some("gemini-3-pro"),
                Some(success),
                Some((1_778_100_001, 0)),
                &[failed, success],
                &[1],
            ),
        )],
        &[(
            1,
            step_metadata_blob(
                Some("gemini-3-pro"),
                Some(success),
                Some((1_778_100_000, 0)),
                &[],
                None,
            ),
        )],
        &[],
    );

    let entries = load_from_fixture(&fixture, true);

    let mut usages = entries
        .iter()
        .map(|entry| (entry.usage.input_tokens, entry.usage.output_tokens))
        .collect::<Vec<_>>();
    usages.sort_unstable();
    assert_eq!(usages, vec![(100, 10), (100, 50)]);
}

#[test]
fn attributes_step_and_retry_usage_to_their_token_models() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = Fixture::new();
    let step_usage = UsageFixture {
        model_id: Some(246),
        input_tokens: 10,
        total_output_tokens: 5,
        visible_output_tokens: 5,
        response_id: Some("step-pro"),
        ..UsageFixture::default()
    };
    let retry_usage = UsageFixture {
        model_id: Some(290),
        input_tokens: 20,
        total_output_tokens: 4,
        visible_output_tokens: 4,
        response_id: Some("retry-opus"),
        ..UsageFixture::default()
    };
    create_database(
        &fixture.path("conversations/session.db"),
        &[(1, metadata_blob(Some("gemini-2.5-flash"), None, None, &[]))],
        &[(
            1,
            step_metadata_blob(
                None,
                Some(step_usage),
                Some((1_778_400_000, 0)),
                &[retry_usage],
                None,
            ),
        )],
        &[],
    );

    Environment::with_vars([(
        super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
        Some(OsString::from(fixture.root())),
    )])
    .run(|| {
        let pricing = PricingMap::load_embedded();
        let entries = load_entries(&shared(true), &pricing).unwrap();

        assert_eq!(entries.len(), 2);
        let step = entries
            .iter()
            .find(|entry| entry.message_id.as_deref() == Some("step-pro"))
            .unwrap();
        let retry = entries
            .iter()
            .find(|entry| entry.message_id.as_deref() == Some("retry-opus"))
            .unwrap();
        assert_eq!(step.model.as_deref(), Some("gemini-2.5-pro"));
        assert_eq!(retry.model.as_deref(), Some("claude-opus-4"));
        let step_pricing = pricing.find("gemini-2.5-pro").unwrap();
        let retry_pricing = pricing.find("claude-opus-4").unwrap();
        let expected_step_cost = 10.0 * step_pricing.input + 5.0 * step_pricing.output;
        let expected_retry_cost = 20.0 * retry_pricing.input + 4.0 * retry_pricing.output;
        assert!((step.cost - expected_step_cost).abs() < 1e-15);
        assert!((retry.cost - expected_retry_cost).abs() < 1e-15);
        assert!(step.missing_pricing_model.is_none());
        assert!(retry.missing_pricing_model.is_none());
    })
}

#[test]
fn maps_required_aliases_and_keeps_unprovided_models_unprefixed() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = Fixture::new();
    let aliases = [
        "gemini-3-flash-agent",
        "gemini-3-flash-a",
        "gemini-3-flash-b",
        "gemini-3.6-flash",
        "gemini-unpriced-model",
    ];
    let rows = aliases
        .iter()
        .enumerate()
        .map(|(index, model)| {
            (
                i64::try_from(index).unwrap(),
                metadata_blob(
                    Some(model),
                    Some(UsageFixture {
                        input_tokens: 1,
                        total_output_tokens: 1,
                        visible_output_tokens: 1,
                        response_id: Some(match index {
                            0 => "alias-0",
                            1 => "alias-1",
                            2 => "alias-2",
                            3 => "alias-3",
                            _ => "alias-4",
                        }),
                        ..UsageFixture::default()
                    }),
                    Some((1_778_200_000 + index as u64, 0)),
                    &[],
                ),
            )
        })
        .collect::<Vec<_>>();
    create_database(&fixture.path("conversations/session.db"), &rows, &[], &[]);

    let entries = load_from_fixture(&fixture, true);

    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.model.as_deref())
            .collect::<Vec<_>>(),
        vec![
            Some("gemini-3.5-flash-high"),
            Some("gemini-3.5-flash-high"),
            Some("gemini-3.5-flash-high"),
            Some("gemini-3.6-flash"),
            Some("gemini-unpriced-model"),
        ]
    );
    assert!(entries.iter().all(|entry| {
        !entry
            .missing_pricing_model
            .as_deref()
            .unwrap_or_default()
            .starts_with("google/")
    }));
    assert!(entries[3].missing_pricing_model.is_none());
    assert_eq!(
        entries[4].missing_pricing_model.as_deref(),
        Some("gemini-unpriced-model")
    );
}

#[test]
fn uses_trajectory_timestamp_when_generation_timestamp_is_missing() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = Fixture::new();
    create_database(
        &fixture.path("conversations/session.db"),
        &[(
            1,
            metadata_blob(
                Some("gemini-3-pro"),
                Some(UsageFixture {
                    input_tokens: 2,
                    total_output_tokens: 3,
                    visible_output_tokens: 3,
                    ..UsageFixture::default()
                }),
                None,
                &[],
            ),
        )],
        &[],
        &[trajectory_metadata_blob(1_778_300_000, 456_000_000)],
    );

    let entries = load_from_fixture(&fixture, true);

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].date.to_string(), "2026-05-09");
    assert_eq!(
        entries[0].timestamp,
        ccusage_core::parse_ts_timestamp("2026-05-09T04:13:20.456Z").unwrap()
    );
}

#[test]
fn untimed_database_rows_are_diagnosed_without_using_file_mtime() {
    let fixture = Fixture::new();
    let usage = Some(UsageFixture {
        input_tokens: 1,
        ..UsageFixture::default()
    });
    create_database(
        &fixture.path("conversations/session.db"),
        &[
            (1, metadata_blob(Some("test-model"), usage, None, &[])),
            (
                2,
                metadata_blob(Some("test-model"), usage, Some((1_778_300_000, 0)), &[]),
            ),
        ],
        &[],
        &[],
    );
    Environment::with_vars([(
        super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
        Some(OsString::from(fixture.root())),
    )])
    .run(|| {
        let (entries, diagnostics) =
            load_with_diagnostics(&shared(true), &PricingMap::default()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].timestamp.as_millis(), 1_778_300_000_000);
        assert_eq!(diagnostics.skipped(SkipKind::InvalidTimestamp), 1);
    })
}

#[test]
fn untimed_step_usage_keeps_the_linked_generation_timestamp() {
    let fixture = Fixture::new();
    let usage = UsageFixture {
        input_tokens: 10,
        cache_read_tokens: 20,
        response_id: Some("response"),
        ..UsageFixture::default()
    };
    create_database(
        &fixture.path("conversations/session.db"),
        &[(
            1,
            metadata_blob_with_steps(
                Some("test-model"),
                Some(UsageFixture {
                    input_tokens: 1,
                    cache_read_tokens: 0,
                    ..usage
                }),
                Some((1_778_300_000, 0)),
                &[],
                &[1],
            ),
        )],
        &[(
            1,
            step_metadata_blob(Some("test-model"), Some(usage), None, &[], None),
        )],
        &[],
    );
    Environment::with_vars([(
        super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
        Some(OsString::from(fixture.root())),
    )])
    .run(|| {
        let (entries, diagnostics) =
            load_with_diagnostics(&shared(true), &PricingMap::default()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].usage.input_tokens, 10);
        assert_eq!(entries[0].usage.cache_read_input_tokens, 20);
        assert_eq!(entries[0].timestamp.as_millis(), 1_778_300_000_000);
        assert_eq!(diagnostics.skipped(SkipKind::InvalidTimestamp), 0);
    })
}

#[test]
fn snapshots_production_reports_for_focused_periods() {
    let fixture = Fixture::new();
    create_database(
        &fixture.path("first/conversations/first.db"),
        &[(
            1,
            metadata_blob(
                Some("gemini-3-pro"),
                Some(UsageFixture {
                    input_tokens: 100,
                    total_output_tokens: 40,
                    visible_output_tokens: 30,
                    reasoning_tokens: 10,
                    cache_creation_tokens: 5,
                    cache_read_tokens: 20,
                    response_id: Some("snapshot-first"),
                    ..UsageFixture::default()
                }),
                Some((1_778_000_000, 0)),
                &[],
            ),
        )],
        &[],
        &[],
    );
    create_database(
        &fixture.path("second/conversations/second.db"),
        &[(
            1,
            metadata_blob(
                Some("gemini-3.6-flash"),
                Some(UsageFixture {
                    input_tokens: 200,
                    total_output_tokens: 50,
                    visible_output_tokens: 50,
                    response_id: Some("snapshot-second"),
                    ..UsageFixture::default()
                }),
                Some((1_780_876_800, 0)),
                &[],
            ),
        )],
        &[],
        &[],
    );
    create_database(
        &fixture.path("third/conversations/third.db"),
        &[
            (
                1,
                metadata_blob(
                    None,
                    Some(UsageFixture {
                        model_id: Some(1402),
                        input_tokens: 300,
                        total_output_tokens: 60,
                        visible_output_tokens: 60,
                        response_id: Some("snapshot-opus-id"),
                        ..UsageFixture::default()
                    }),
                    Some((1_781_481_600, 0)),
                    &[],
                ),
            ),
            (
                2,
                metadata_blob(
                    Some("model_placeholder_m402"),
                    Some(UsageFixture {
                        input_tokens: 400,
                        total_output_tokens: 80,
                        visible_output_tokens: 80,
                        response_id: Some("snapshot-opus-placeholder"),
                        ..UsageFixture::default()
                    }),
                    Some((1_781_481_660, 0)),
                    &[],
                ),
            ),
        ],
        &[],
        &[],
    );
    let pricing = snapshot_pricing();
    let entries = load_from_fixture_with_pricing(&fixture, true, &pricing);
    for response_id in ["snapshot-opus-id", "snapshot-opus-placeholder"] {
        let entry = entries
            .iter()
            .find(|entry| entry.message_id.as_deref() == Some(response_id))
            .unwrap();
        assert_eq!(entry.model.as_deref(), Some("claude-opus-5-5"));
        assert!(entry.cost > 0.0);
        assert!(entry.missing_pricing_model.is_none());
    }
    let reports = crate::cli::AgentReportKind::ALL
        .into_iter()
        .map(|kind| {
            let rows = ccusage_adapter_common::source::EntrySource::summarize(&crate::SOURCE, &entries, ccusage_adapter_common::source::SummaryKind::for_report(kind, ccusage_core::cli::WeekDay::Monday)).unwrap();
            (
                format!("{kind:?}").to_lowercase(),
                json!({
                "json": ccusage_adapter_common::source::EntrySource::report_json(&crate::SOURCE, &rows, kind),
                "table": rows.iter().map(|row| json!({
                    "firstColumn": ccusage_core::first_column(kind),
                    "period": ccusage_core::summary_period(row),
                        "models": row.models_used,
                        "input": row.input_tokens,
                        "output": row.output_tokens,
                        "cacheCreate": row.cache_creation_tokens,
                        "cacheRead": row.cache_read_tokens,
                        "total": row.total_tokens(),
                        "cost": row.total_cost,
                    })).collect::<Vec<_>>(),
                }),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();

    insta::assert_json_snapshot!(reports);
}

#[test]
fn prefers_explicit_model_name_over_placeholder_for_unknown_ids() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = Fixture::new();
    let cases = [
        (1401, "claude-opus-5-5-medium", "unknown-opus"),
        (1404, "claude-sonnet-5-5-medium", "unknown-sonnet"),
        (1427, "gemini-4-carbon", "unknown-carbon-high"),
        (1437, "gemini-4-carbon", "unknown-carbon-medium"),
    ];
    let rows = cases
        .iter()
        .enumerate()
        .map(|(idx, (model_id, model, response_id))| {
            (
                i64::try_from(idx).unwrap(),
                metadata_blob(
                    Some(model),
                    Some(UsageFixture {
                        model_id: Some(*model_id),
                        input_tokens: 60,
                        total_output_tokens: 10,
                        cache_read_tokens: 40,
                        visible_output_tokens: 10,
                        response_id: Some(response_id),
                        ..UsageFixture::default()
                    }),
                    Some((1_781_481_600 + u64::try_from(idx).unwrap(), 0)),
                    &[UsageFixture {
                        model_id: Some(1499),
                        input_tokens: 5,
                        total_output_tokens: 1,
                        visible_output_tokens: 1,
                        response_id: Some("unknown-retry"),
                        ..UsageFixture::default()
                    }],
                ),
            )
        })
        .collect::<Vec<_>>();
    create_database(&fixture.path("conversations/session.db"), &rows, &[], &[]);

    let entries = load_from_fixture(&fixture, true);

    for (_, model, response_id) in cases {
        let entry = entries
            .iter()
            .find(|entry| entry.message_id.as_deref() == Some(response_id))
            .unwrap();
        assert_eq!(entry.model.as_deref(), Some(model));
        assert!(entry.cost > 0.0);
        assert!(entry.missing_pricing_model.is_none());
    }
    let retry = entries
        .iter()
        .find(|entry| entry.message_id.as_deref() == Some("unknown-retry"))
        .unwrap();
    assert_eq!(retry.model.as_deref(), Some("model_placeholder_m499"));
}

#[test]
fn prices_gemini_2_5_flash_thinking_at_the_gemini_2_5_flash_rate() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = Fixture::new();
    create_database(
        &fixture.path("conversations/session.db"),
        &[(
            0,
            metadata_blob(
                None,
                Some(UsageFixture {
                    model_id: Some(313),
                    input_tokens: 60,
                    total_output_tokens: 10,
                    visible_output_tokens: 10,
                    ..UsageFixture::default()
                }),
                Some((1_781_481_600, 0)),
                &[],
            ),
        )],
        &[],
        &[],
    );

    let entries = load_from_fixture(&fixture, true);

    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].model.as_deref(),
        Some("gemini-2.5-flash-thinking")
    );
    assert!(entries[0].missing_pricing_model.is_none());
    assert!(
        (entries[0].cost - (60.0 * 0.3e-6 + 10.0 * 2.5e-6)).abs() < 1e-12,
        "{}",
        entries[0].cost
    );
}

#[test]
fn keeps_alias_mapped_ids_over_explicit_model_name() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = Fixture::new();
    create_database(
        &fixture.path("conversations/session.db"),
        &[(
            0,
            metadata_blob(
                Some("gemini-3-flash-agent"),
                Some(UsageFixture {
                    model_id: Some(1187),
                    input_tokens: 60,
                    total_output_tokens: 10,
                    visible_output_tokens: 10,
                    ..UsageFixture::default()
                }),
                Some((1_781_481_600, 0)),
                &[],
            ),
        )],
        &[],
        &[],
    );

    let entries = load_from_fixture(&fixture, true);

    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].model.as_deref(),
        Some("gemini-3.5-flash-extra-low")
    );
    assert!(entries[0].cost > 0.0);
    assert!(entries[0].missing_pricing_model.is_none());
}

#[test]
fn keeps_explicit_model_name_when_merging_unnamed_step_copy() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = Fixture::new();
    let usage = UsageFixture {
        model_id: Some(1401),
        input_tokens: 60,
        total_output_tokens: 10,
        visible_output_tokens: 10,
        response_id: Some("shared-response"),
        ..UsageFixture::default()
    };
    create_database(
        &fixture.path("conversations/session.db"),
        &[(
            0,
            metadata_blob(
                Some("claude-opus-5-5-medium"),
                Some(usage),
                Some((1_781_481_600, 0)),
                &[],
            ),
        )],
        &[(
            0,
            step_metadata_blob(None, Some(usage), Some((1_781_481_600, 0)), &[], Some(1)),
        )],
        &[],
    );

    let entries = load_from_fixture(&fixture, true);

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].model.as_deref(), Some("claude-opus-5-5-medium"));
}

#[test]
fn prices_flash_agent_routing_names_as_the_mapped_flash_model() {
    ccusage_test_support::install_pricing_snapshots();
    let pricing = PricingMap::load_embedded();
    for model in [
        "gemini-3-flash-agent",
        "gemini-3-flash-agent-a",
        "gemini-3-flash-agent-b",
        "gemini-3-flash-a",
        "gemini-3-flash-b",
    ] {
        let fixture = routing_model_fixture(model);
        let entries = load_from_fixture_with_pricing(&fixture, true, &pricing);
        assert_eq!(entries.len(), 1, "{model}");
        assert_eq!(entries[0].model.as_deref(), Some("gemini-3.5-flash-high"));
        assert!(
            (entries[0].cost - 0.00279).abs() < 1e-12,
            "{model}: {}",
            entries[0].cost
        );
    }
}

#[test]
fn prices_pro_agent_routing_name_as_the_mapped_pro_model_online() {
    ccusage_test_support::install_pricing_snapshots();
    ccusage_core::pricing::set_json_fetcher(|url, load| {
        let live = if url.ends_with("/model_prices_and_context_window.json") {
            r#"{
                    "gemini-pro": {"input_cost_per_token": 0.0000005, "output_cost_per_token": 0.0000015},
                    "gemini-3.1-pro": {"input_cost_per_token": 0.000002, "output_cost_per_token": 0.000012, "cache_read_input_token_cost": 0.0000002}
                }"#
        } else {
            return Err(std::io::Error::other("no models.dev fixture"));
        };
        assert!(load(live), "{url}");
        Ok(())
    });
    let pricing = PricingMap::load_with_overrides(false, false, []);
    assert_eq!(pricing.find("gemini-pro").unwrap().input, 0.0000005);
    let fixture = routing_model_fixture("gemini-pro-agent");
    let entries = load_from_fixture_with_pricing(&fixture, true, &pricing);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].model.as_deref(), Some("gemini-3.1-pro"));
    assert!(
        (entries[0].cost - 0.00372).abs() < 1e-12,
        "{}",
        entries[0].cost
    );
}

fn routing_model_fixture(model: &str) -> Fixture {
    let fixture = Fixture::new();
    fixture.write_file("conversations/session.db", "");
    create_database(
        &fixture.path("conversations/session.db"),
        &[(
            1,
            metadata_blob(
                Some(model),
                Some(UsageFixture {
                    input_tokens: 940,
                    total_output_tokens: 120,
                    visible_output_tokens: 120,
                    cache_read_tokens: 2_000,
                    ..UsageFixture::default()
                }),
                Some((1_778_000_000, 0)),
                &[],
            ),
        )],
        &[],
        &[],
    );
    fixture
}

#[test]
fn prices_the_logged_model_before_its_normalized_form() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = Fixture::new();
    fixture.write_file("conversations/session.db", "");
    create_database(
        &fixture.path("conversations/session.db"),
        &[(
            1,
            metadata_blob(
                Some("GPT-Logged-Model"),
                Some(UsageFixture {
                    input_tokens: 100,
                    ..UsageFixture::default()
                }),
                Some((1_778_000_000, 0)),
                &[],
            ),
        )],
        &[],
        &[],
    );
    let mut pricing = PricingMap::default();
    pricing.load_json(
        r#"{
                "GPT-Logged-Model":{"input_cost_per_token":1,"output_cost_per_token":1},
                "gpt-logged-model":{"input_cost_per_token":2,"output_cost_per_token":2}
            }"#,
    );
    let entries = load_from_fixture_with_pricing(&fixture, true, &pricing);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].model.as_deref(), Some("gpt-logged-model"));
    assert_eq!(entries[0].cost, 100.0);
}

#[test]
fn skips_zero_byte_databases_and_reports_valid_entries() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = Fixture::new();
    fixture.write_file("conversations/empty.db", "");
    create_database(
        &fixture.path("conversations/session.db"),
        &[(
            1,
            metadata_blob(
                Some("gemini-3-pro"),
                Some(UsageFixture {
                    input_tokens: 10,
                    total_output_tokens: 5,
                    visible_output_tokens: 5,
                    response_id: Some("valid-response"),
                    ..UsageFixture::default()
                }),
                Some((1_778_000_000, 0)),
                &[],
            ),
        )],
        &[],
        &[],
    );

    let entries = load_from_fixture(&fixture, true);

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].message_id.as_deref(), Some("valid-response"));
}

#[test]
fn returns_empty_report_when_only_zero_byte_databases_exist() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = Fixture::new();
    let database = fixture.write_file("conversations/empty.db", "");

    assert!(matches!(
        parse_sqlite_file(&database, &Diagnostics::new("antigravity")).unwrap(),
        ParsedDatabase::EmptySnapshot
    ));

    let entries = load_from_fixture(&fixture, true);

    assert!(entries.is_empty());
}

#[test]
fn returns_empty_report_for_fresh_install_without_conversations_directory() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = Fixture::new();
    let summaries_path = fixture.path(".gemini/antigravity-cli/conversation_summaries.db");
    fixture.create_dir_all(".gemini/antigravity-cli");
    let connection = sqlite::open(&summaries_path).unwrap();
    connection
        .execute("CREATE TABLE summaries (id INTEGER PRIMARY KEY)")
        .unwrap();
    Environment::with_vars([
        (super::super::paths::ANTIGRAVITY_DATA_DIR_ENV, None),
        ("HOME", Some(OsString::from(fixture.root()))),
        ("USERPROFILE", Some(OsString::from(fixture.root()))),
    ])
    .run(|| {
        assert!(
            super::super::paths::conversation_db_paths(&Diagnostics::new("antigravity"), false)
                .unwrap()
                .is_empty()
        );

        let entries = load_entries(&shared(true), &PricingMap::load_embedded()).unwrap();

        assert!(entries.is_empty());
    })
}

#[test]
fn skips_malformed_database_alongside_empty_databases() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = Fixture::new();
    fixture.write_file("conversations/empty.db", "");
    create_database(
        &fixture.path("conversations/malformed.db"),
        &[(1, vec![0x0a, 0x01, 0x80])],
        &[],
        &[],
    );
    Environment::with_vars([(
        super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
        Some(OsString::from(fixture.root())),
    )])
    .run(|| {
        let diagnostics = skipped_databases();

        assert!(
            diagnostics
                .warnings(&SharedArgs {
                    debug: true,
                    ..shared(true)
                })
                .contains(&format!(
                    "Skipping uninitialized Antigravity database '{}': empty SQLite snapshot",
                    fixture.path("conversations/empty.db").display()
                ))
        );
        assert_skipped(
            &diagnostics,
            SkipKind::InvalidData,
            "invalid data",
            &fixture.path("conversations/malformed.db"),
        );
    })
}

#[test]
fn skips_database_open_and_query_errors() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = Fixture::new();
    let not_a_database = fixture.write_file("conversations/not-a-db.db", "not sqlite");
    Environment::with_vars([(
        super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
        Some(OsString::from(fixture.root())),
    )])
    .run(|| {
        assert_skipped(
            &skipped_databases(),
            SkipKind::InvalidData,
            "invalid data",
            &not_a_database,
        );
    })
}

#[test]
fn notes_database_without_generation_table_only_under_debug() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = Fixture::new();
    let db_path = fixture.path("conversation_summaries.db");
    let connection = sqlite::open(&db_path).unwrap();
    connection
        .execute("CREATE TABLE summaries (id INTEGER PRIMARY KEY)")
        .unwrap();
    Environment::with_vars([(
        super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
        Some(OsString::from(fixture.root())),
    )])
    .run(|| {
        let diagnostics = skipped_databases();

        assert_eq!(diagnostics.skipped(SkipKind::Invariant), 0);
        assert!(diagnostics.warnings(&shared(true)).is_empty());
        assert!(
            diagnostics
                .warnings(&SharedArgs {
                    debug: true,
                    ..shared(true)
                })
                .contains(&format!(
                    "Skipping Antigravity database '{}': no gen_metadata table",
                    db_path.display()
                ))
        );
    })
}

#[test]
fn skips_a_conversation_database_with_steps_but_no_generation_table() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = Fixture::new();
    fixture.create_dir_all("conversations");
    let db_path = fixture.path("conversations/renamed.db");
    let connection = sqlite::open(&db_path).unwrap();
    connection
        .execute("CREATE TABLE steps (idx INTEGER PRIMARY KEY, metadata BLOB)")
        .unwrap();
    Environment::with_vars([(
        super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
        Some(OsString::from(fixture.root())),
    )])
    .run(|| {
        assert_skipped(
            &skipped_databases(),
            SkipKind::Invariant,
            "unexpected shape",
            &db_path,
        );
    })
}

#[test]
fn skips_database_with_invalid_optional_table_schema() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = Fixture::new();
    let db_path = fixture.path("conversations/bad-steps.db");
    fixture.create_dir_all("conversations");
    let connection = sqlite::open(&db_path).unwrap();
    connection
        .execute("CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB NOT NULL)")
        .unwrap();
    connection
        .execute("CREATE TABLE steps (idx INTEGER PRIMARY KEY, wrong BLOB)")
        .unwrap();
    connection
        .execute("CREATE TABLE trajectory_metadata_blob (data BLOB)")
        .unwrap();
    let data = metadata_blob(
        Some("gemini-3-pro"),
        Some(UsageFixture {
            input_tokens: 1,
            total_output_tokens: 1,
            visible_output_tokens: 1,
            ..UsageFixture::default()
        }),
        Some((1_778_000_000, 0)),
        &[],
    );
    let mut statement = connection
        .prepare("INSERT INTO gen_metadata (idx, data) VALUES (?1, ?2)")
        .unwrap();
    statement.bind((1, 1_i64)).unwrap();
    statement.bind((2, data.as_slice())).unwrap();
    statement.next().unwrap();
    Environment::with_vars([(
        super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
        Some(OsString::from(fixture.root())),
    )])
    .run(|| {
        assert_skipped(
            &skipped_databases(),
            SkipKind::Invariant,
            "unexpected shape",
            &db_path,
        );
    })
}

#[test]
fn skips_database_with_malformed_metadata() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = Fixture::new();
    create_database(
        &fixture.path("conversations/malformed.db"),
        &[(1, vec![0x0a, 0x01, 0x80])],
        &[],
        &[],
    );
    Environment::with_vars([(
        super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
        Some(OsString::from(fixture.root())),
    )])
    .run(|| {
        assert_skipped(
            &skipped_databases(),
            SkipKind::InvalidData,
            "invalid data",
            &fixture.path("conversations/malformed.db"),
        );
    })
}

#[test]
fn structured_loads_tally_skipped_databases_without_their_content() {
    ccusage_test_support::install_pricing_snapshots();
    use ccusage_adapter_common::source::LoadContext;
    use ccusage_core::{ReportWindow, cli::AgentReportKind};

    let fixture = Fixture::new();
    fixture.write_file("conversations/broken.db", "SECRET_CANARY");
    Environment::with_vars([(
        super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
        Some(OsString::from(fixture.root())),
    )])
    .run(|| {
        let shared = shared(true);
        let pricing = PricingMap::load_embedded();
        let window = ReportWindow::from_shared(&shared);
        let diagnostics = Diagnostics::new("antigravity");
        let cx = LoadContext {
            shared: &shared,
            kind: AgentReportKind::Daily,
            window: &window,
            pricing: ccusage_adapter_common::source::ReportPricing::Supplied(&pricing),
            options: SourceOptions::default(),
            diagnostics: &diagnostics,
        };

        assert!(load_entries_for_context(&cx).unwrap().is_empty());
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        assert!(
            !diagnostics
                .warnings(&SharedArgs {
                    debug: true,
                    ..shared
                })
                .join("\n")
                .contains("SECRET_CANARY")
        );
    })
}
