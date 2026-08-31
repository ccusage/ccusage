use std::collections::HashMap;

use ccusage_adapter_common::read_files_parallel;

use crate::{LoadedEntry, PricingMap, Result, cli::SharedArgs, parse_tz};

use super::{
    parser::{AntigravityUsageEvent, event_to_loaded, merge_usage_event, parse_sqlite_file},
    paths::conversation_db_paths,
};

/// Loads Antigravity generation metadata from all discovered conversation databases.
pub fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent("Antigravity"),
        shared.json,
        || load_entries_inner(shared, pricing),
    )
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let timezone = parse_tz(shared.timezone.as_deref());
    let database_paths = conversation_db_paths()?;
    let parsed = read_files_parallel(&database_paths, shared.single_thread, parse_sqlite_file);
    let mut events = deduplicate_events(
        parsed
            .into_iter()
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten(),
    );
    events.sort_by_key(|event| event.timestamp);
    Ok(events
        .into_iter()
        .map(|event| event_to_loaded(event, timezone.as_ref(), shared.mode, pricing))
        .collect())
}

fn deduplicate_events(
    events: impl IntoIterator<Item = AntigravityUsageEvent>,
) -> Vec<AntigravityUsageEvent> {
    let mut slots: Vec<Option<AntigravityUsageEvent>> = Vec::new();
    let mut identity_indexes = HashMap::new();
    for event in events {
        let mut matching_indexes = event
            .identities
            .iter()
            .filter_map(|identity| identity_indexes.get(identity).copied())
            .collect::<Vec<_>>();
        matching_indexes.sort_unstable();
        matching_indexes.dedup();
        let Some(target_index) = matching_indexes.first().copied() else {
            let target_index = slots.len();
            for identity in &event.identities {
                identity_indexes.insert(identity.clone(), target_index);
            }
            slots.push(Some(event));
            continue;
        };

        for duplicate_index in matching_indexes.into_iter().skip(1) {
            if let Some(duplicate) = slots[duplicate_index].take() {
                merge_usage_event(
                    slots[target_index]
                        .as_mut()
                        .expect("deduplication target must exist"),
                    duplicate,
                );
            }
        }
        merge_usage_event(
            slots[target_index]
                .as_mut()
                .expect("deduplication target must exist"),
            event,
        );
        if let Some(target) = slots[target_index].as_ref() {
            for identity in &target.identities {
                identity_indexes.insert(identity.clone(), target_index);
            }
        }
    }
    slots.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use ccusage_test_support::{EnvVarsGuard, Fixture};
    use serde_json::json;

    use super::*;
    use crate::parser::test_support::{
        UsageFixture, create_database, metadata_blob, step_metadata_blob, trajectory_metadata_blob,
    };

    fn shared(single_thread: bool) -> SharedArgs {
        SharedArgs {
            mode: crate::cli::CostMode::Calculate,
            single_thread,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        }
    }

    fn load_from_fixture(fixture: &Fixture, single_thread: bool) -> Vec<LoadedEntry> {
        let _guard = EnvVarsGuard::set_many([(
            super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
            Some(OsString::from(fixture.root())),
        )]);
        load_entries(&shared(single_thread), &PricingMap::load_embedded()).unwrap()
    }

    #[test]
    fn loads_real_schema_rows_with_continuations_and_token_buckets() {
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
        assert_eq!(entries[0].data.message.id.as_deref(), Some("response-1"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 400);
        assert_eq!(
            entries[0].data.message.usage.cache_creation_input_tokens,
            30
        );
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 500);
        assert_eq!(entries[0].data.message.usage.output_tokens, 150);
        assert_eq!(entries[0].extra_total_tokens, 50);
        assert_eq!(entries[1].data.message.usage.input_tokens, 300);
        assert_eq!(entries[1].data.message.usage.output_tokens, 75);
        assert_eq!(entries[1].extra_total_tokens, 25);
        assert!(entries[0].cost > 0.0);
    }

    #[test]
    fn decodes_model_id_without_counting_it_as_input_tokens() {
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
        assert_eq!(entries[0].data.message.usage.input_tokens, 4);
    }

    #[test]
    fn parallel_and_single_threaded_database_reads_match() {
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
        let _guard = EnvVarsGuard::set_many([(
            super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
            Some(OsString::from(override_value)),
        )]);

        let sequential = load_entries(&shared(true), &PricingMap::load_embedded()).unwrap();
        let parallel = load_entries(&shared(false), &PricingMap::load_embedded()).unwrap();

        assert_eq!(
            sequential
                .iter()
                .map(|entry| entry.data.message.id.clone())
                .collect::<Vec<_>>(),
            parallel
                .iter()
                .map(|entry| entry.data.message.id.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(sequential.len(), 2);
    }

    #[test]
    fn deduplicates_response_ids_across_databases() {
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
        let _guard = EnvVarsGuard::set_many([(
            super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
            Some(OsString::from(override_value)),
        )]);

        let entries = load_entries(&shared(true), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].data.message.id.as_deref(),
            Some("shared-response")
        );
        assert_eq!(entries[0].data.message.usage.input_tokens, 200);
        assert_eq!(entries[0].data.message.usage.output_tokens, 350);
    }

    #[test]
    fn collects_steps_and_retries_and_deduplicates_source_copies() {
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
        assert_eq!(entries[0].data.message.usage.input_tokens, 100);
        assert_eq!(entries[0].data.message.usage.cache_creation_input_tokens, 5);
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 7);
        assert_eq!(entries[0].data.message.usage.output_tokens, 40);
        assert_eq!(entries[0].extra_total_tokens, 10);
        assert_eq!(entries[0].model.as_deref(), Some("gemini-3-pro"));
        assert_eq!(entries[1].model.as_deref(), Some("gemini-3.5-flash-high"));
        assert_eq!(entries[1].data.message.usage.input_tokens, 11);
    }

    #[test]
    fn attributes_step_and_retry_usage_to_their_token_models() {
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

        let _guard = EnvVarsGuard::set_many([(
            super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
            Some(OsString::from(fixture.root())),
        )]);
        let pricing = PricingMap::load_embedded();
        let entries = load_entries(&shared(true), &pricing).unwrap();

        assert_eq!(entries.len(), 2);
        let step = entries
            .iter()
            .find(|entry| entry.data.message.id.as_deref() == Some("step-pro"))
            .unwrap();
        let retry = entries
            .iter()
            .find(|entry| entry.data.message.id.as_deref() == Some("retry-opus"))
            .unwrap();
        assert_eq!(step.model.as_deref(), Some("gemini-2.5-pro"));
        assert_eq!(retry.model.as_deref(), Some("claude-4-opus"));
        let step_pricing = pricing.find("gemini-2.5-pro").unwrap();
        let retry_pricing = pricing.find("claude-4-opus").unwrap();
        let expected_step_cost = 10.0 * step_pricing.input + 5.0 * step_pricing.output;
        let expected_retry_cost = 20.0 * retry_pricing.input + 4.0 * retry_pricing.output;
        assert!((step.cost - expected_step_cost).abs() < 1e-15);
        assert!((retry.cost - expected_retry_cost).abs() < 1e-15);
        assert!(step.missing_pricing_model.is_none());
        assert!(retry.missing_pricing_model.is_none());
    }

    #[test]
    fn maps_required_aliases_and_keeps_unprovided_models_unprefixed() {
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
                    index as i64,
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
        assert_eq!(entries[0].date, "2026-05-09");
        assert_eq!(entries[0].data.timestamp, "2026-05-09T04:13:20.456Z");
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
        let entries = load_from_fixture(&fixture, true);
        let periods = [
            crate::cli::AgentReportKind::Daily,
            crate::cli::AgentReportKind::Monthly,
            crate::cli::AgentReportKind::Session,
        ];
        let reports = periods
            .into_iter()
            .map(|kind| {
                let rows = crate::report::summarize_entries(&entries, kind).unwrap();
                (
                    format!("{kind:?}").to_lowercase(),
                    json!({
                    "json": crate::report::report_from_rows(&rows, kind),
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
    fn propagates_database_open_and_query_errors() {
        let fixture = Fixture::new();
        let not_a_database = fixture.write_file("conversations/not-a-db.db", "not sqlite");
        let _guard = EnvVarsGuard::set_many([(
            super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
            Some(OsString::from(fixture.root())),
        )]);

        let error = load_entries(&shared(true), &PricingMap::load_embedded()).unwrap_err();
        let message = error.to_string();

        assert!(message.contains(not_a_database.to_string_lossy().as_ref()));
        assert!(message.contains("open") || message.contains("database"));
    }

    #[test]
    fn propagates_missing_generation_table_errors() {
        let fixture = Fixture::new();
        let db_path = fixture.path("conversations/missing-table.db");
        let _ = fixture.create_dir_all("conversations");
        let connection = sqlite::open(&db_path).unwrap();
        connection
            .execute("CREATE TABLE other (id INTEGER)")
            .unwrap();
        let _guard = EnvVarsGuard::set_many([(
            super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
            Some(OsString::from(fixture.root())),
        )]);

        let error = load_entries(&shared(true), &PricingMap::load_embedded()).unwrap_err();

        assert!(error.to_string().contains("gen_metadata"));
    }

    #[test]
    fn propagates_present_optional_table_schema_errors() {
        let fixture = Fixture::new();
        let db_path = fixture.path("conversations/bad-steps.db");
        let _ = fixture.create_dir_all("conversations");
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
        let _guard = EnvVarsGuard::set_many([(
            super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
            Some(OsString::from(fixture.root())),
        )]);

        let error = load_entries(&shared(true), &PricingMap::load_embedded()).unwrap_err();

        assert!(error.to_string().contains("steps"));
    }

    #[test]
    fn propagates_malformed_metadata_errors() {
        let fixture = Fixture::new();
        create_database(
            &fixture.path("conversations/malformed.db"),
            &[(1, vec![0x0a, 0x01, 0x80])],
            &[],
            &[],
        );
        let _guard = EnvVarsGuard::set_many([(
            super::super::paths::ANTIGRAVITY_DATA_DIR_ENV,
            Some(OsString::from(fixture.root())),
        )]);

        let error = load_entries(&shared(true), &PricingMap::load_embedded()).unwrap_err();

        assert!(error.to_string().contains("parse Antigravity metadata"));
    }
}
