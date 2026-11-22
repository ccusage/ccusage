use std::sync::Arc;

use super::*;
use crate::{ModelBreakdown, TokenUsageRaw};

#[test]
fn key_target_keeps_project_sessions_separate_without_activity_metadata() {
    let entries = vec![
        loaded_entry(LoadedEntryFixture {
            date: "2026-01-02",
            timestamp: 1_767_316_800_000,
            session_id: "shared",
            project_path: "/a",
            model: None,
            input_tokens: 10,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            extra_total_tokens: 0,
            cost: 0.1,
            credits: None,
            message_count: None,
            version: None,
            missing_pricing_model: None,
        }),
        loaded_entry(LoadedEntryFixture {
            date: "2026-01-02",
            timestamp: 1_767_316_800_000,
            session_id: "shared",
            project_path: "/b",
            model: None,
            input_tokens: 20,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            extra_total_tokens: 0,
            cost: 0.2,
            credits: None,
            message_count: None,
            version: None,
            missing_pricing_model: None,
        }),
    ];
    let rows = summarize_by_key_target(
        &entries,
        |entry| {
            (
                std::sync::Arc::clone(&entry.project_path),
                std::sync::Arc::clone(&entry.session_id),
            )
        },
        |(_, session)| (session.to_string(), None),
        KeyTarget::Session,
    )
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].input_tokens, 10);
    assert_eq!(rows[1].input_tokens, 20);
    assert!(rows.iter().all(|row| row.date.is_none()
        && row.session_id.as_deref() == Some("shared")
        && row.last_activity.is_none()));
}

#[test]
fn snapshots_summarize_by_key_aggregates_counts_costs_and_breakdowns() {
    let entries = vec![
        loaded_entry(LoadedEntryFixture {
            date: "2026-01-02",
            timestamp: 1_767_316_800_000,
            session_id: "session-a",
            project_path: "/workspace/api",
            model: Some("gpt-5.2-codex"),
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_tokens: 10,
            cache_read_tokens: 5,
            extra_total_tokens: 7,
            cost: 0.125,
            credits: Some(1.0),
            message_count: Some(2),
            version: Some("1.0.0"),
            missing_pricing_model: None,
        }),
        loaded_entry(LoadedEntryFixture {
            date: "2026-01-02",
            timestamp: 1_767_320_400_000,
            session_id: "session-b",
            project_path: "/workspace/api",
            model: Some("claude-sonnet-4-20250514"),
            input_tokens: 200,
            output_tokens: 80,
            cache_creation_tokens: 20,
            cache_read_tokens: 8,
            extra_total_tokens: 0,
            cost: 0.25,
            credits: Some(0.5),
            message_count: Some(3),
            version: Some("1.0.1"),
            missing_pricing_model: None,
        }),
        loaded_entry(LoadedEntryFixture {
            date: "2026-01-03",
            timestamp: 1_767_402_000_000,
            session_id: "session-c",
            project_path: "/workspace/web",
            model: None,
            input_tokens: 10,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            extra_total_tokens: 5,
            cost: 0.0,
            credits: None,
            message_count: None,
            version: None,
            missing_pricing_model: None,
        }),
    ];

    let rows = summarize_by_key_target(
        &entries,
        |entry| format!("{}|{}", entry.date, entry.project_path),
        |key| {
            let (date, project) = key.split_once('|').unwrap();
            (date.to_string(), Some(project.to_string()))
        },
        KeyTarget::Date,
    )
    .unwrap();

    insta::assert_json_snapshot!(rows);
}

#[test]
fn snapshots_session_accumulator_latest_metadata_versions_and_timezone() {
    let mut accumulator = SessionAccumulator::default();
    accumulator.add_entry(&loaded_entry(LoadedEntryFixture {
        date: "2026-01-02",
        timestamp: 1_767_316_800_000,
        session_id: "older-session",
        project_path: "/workspace/old",
        model: Some("gpt-5.2-codex"),
        input_tokens: 100,
        output_tokens: 50,
        cache_creation_tokens: 10,
        cache_read_tokens: 5,
        extra_total_tokens: 0,
        cost: 0.125,
        credits: None,
        message_count: Some(1),
        version: Some("1.0.0"),
        missing_pricing_model: None,
    }));
    accumulator.add_entry(&loaded_entry(LoadedEntryFixture {
        date: "2026-01-03",
        timestamp: 1_767_402_000_000,
        session_id: "latest-session",
        project_path: "/workspace/new",
        model: Some("claude-sonnet-4-20250514"),
        input_tokens: 20,
        output_tokens: 10,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        extra_total_tokens: 3,
        cost: 0.25,
        credits: None,
        message_count: Some(4),
        version: Some("1.1.0"),
        missing_pricing_model: None,
    }));

    let row = accumulator.into_summary().unwrap();

    insta::assert_json_snapshot!(row);
}

#[test]
fn snapshots_bucket_aggregation_for_week_boundaries_invalid_dates_and_model_merging() {
    let rows = vec![
        summary_row(SummaryFixture {
            date: Some("2025-12-31"),
            model: "gpt-5.2-codex",
            cost: 0.125,
            input_tokens: 100,
        }),
        summary_row(SummaryFixture {
            date: Some("2026-01-01"),
            model: "claude-sonnet-4-20250514",
            cost: 0.25,
            input_tokens: 200,
        }),
        summary_row(SummaryFixture {
            date: Some("2026-01-05"),
            model: "gpt-5.2-codex",
            cost: 0.5,
            input_tokens: 300,
        }),
        summary_row(SummaryFixture {
            date: Some("not-a-date"),
            model: "unknown-model",
            cost: 0.0,
            input_tokens: 1,
        }),
        summary_row(SummaryFixture {
            date: None,
            model: "ignored-no-date",
            cost: 9.99,
            input_tokens: 999,
        }),
    ];

    let weekly = summarize_bucket(&rows, Bucket::Weekly(WeekDay::Monday));
    let monthly = summarize_bucket(&rows, Bucket::Monthly);

    insta::assert_json_snapshot!(serde_json::json!({
        "weekly": weekly,
        "monthly": monthly,
    }));
}

#[test]
fn tracks_missing_pricing_in_model_breakdowns() {
    let mut accumulator = SessionAccumulator::default();
    accumulator.add_entry(&loaded_entry(LoadedEntryFixture {
        date: "2026-01-02",
        timestamp: 1_767_316_800_000,
        session_id: "session-a",
        project_path: "/workspace/project",
        model: Some("claude-opus-4-9"),
        input_tokens: 100,
        output_tokens: 50,
        cache_creation_tokens: 10,
        cache_read_tokens: 5,
        extra_total_tokens: 0,
        cost: 0.0,
        credits: None,
        message_count: Some(1),
        version: Some("1.0.0"),
        missing_pricing_model: Some("claude-opus-4-9"),
    }));

    let row = accumulator.into_summary().unwrap();

    assert_eq!(row.model_breakdowns[0].model_name, "claude-opus-4-9");
    assert!(row.model_breakdowns[0].missing_pricing);
}

#[test]
fn tracks_stored_cost_in_model_breakdowns() {
    let fixture = |timestamp, model, cost| LoadedEntryFixture {
        date: "2026-01-02",
        timestamp,
        session_id: "session-a",
        project_path: "/workspace/project",
        model: Some(model),
        input_tokens: 100,
        output_tokens: 10,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        extra_total_tokens: 0,
        cost,
        credits: None,
        message_count: None,
        version: None,
        missing_pricing_model: None,
    };
    let stored = LoadedEntry {
        stored_cost: Some(0.5),
        ..loaded_entry(fixture(1_767_316_800_000, "claude-sonnet-4-20250514", 0.5))
    };
    let calculated = loaded_entry(fixture(1_767_316_801_000, "claude-sonnet-4-20250514", 0.25));
    let other = loaded_entry(fixture(1_767_316_802_000, "gpt-5", 0.125));

    let rows = summarize_by_key_target(
        &[stored.clone(), calculated, other, stored],
        |entry| entry.date,
        |key| (key.to_string(), None),
        KeyTarget::Date,
    )
    .unwrap();

    let breakdowns = &rows[0].model_breakdowns;
    assert_eq!(breakdowns[0].model_name, "claude-sonnet-4-20250514");
    assert_eq!(breakdowns[0].cost, 1.25);
    assert_eq!(breakdowns[0].stored_cost, Some(1.0));
    assert_eq!(breakdowns[1].model_name, "gpt-5");
    assert_eq!(breakdowns[1].stored_cost, None);

    let mut next_day = summary_row(SummaryFixture {
        date: Some("2026-01-03"),
        model: "claude-sonnet-4-20250514",
        cost: 0.25,
        input_tokens: 100,
    });
    next_day.model_breakdowns[0].stored_cost = Some(0.25);
    let monthly = summarize_bucket(&[rows[0].clone(), next_day], Bucket::Monthly);

    assert_eq!(monthly[0].model_breakdowns[0].stored_cost, Some(1.25));
    assert_eq!(monthly[0].model_breakdowns[1].stored_cost, None);
}

#[test]
fn applies_model_aliases_to_model_breakdowns() {
    let _aliases = crate::model_aliases::set_model_aliases_for_tests([
        ("private-alpha", "gpt-5.5"),
        ("another-private-alpha", "gpt-5.5"),
    ]);
    let mut accumulator = SessionAccumulator::default();
    accumulator.add_entry(&loaded_entry(LoadedEntryFixture {
        date: "2026-01-02",
        timestamp: 1_767_316_800_000,
        session_id: "session-a",
        project_path: "/workspace/project",
        model: Some("private-alpha"),
        input_tokens: 20,
        output_tokens: 5,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        extra_total_tokens: 0,
        cost: 0.02,
        credits: None,
        message_count: Some(1),
        version: Some("1.0.0"),
        missing_pricing_model: None,
    }));
    accumulator.add_entry(&loaded_entry(LoadedEntryFixture {
        date: "2026-01-02",
        timestamp: 1_767_316_801_000,
        session_id: "session-a",
        project_path: "/workspace/project",
        model: Some("another-private-alpha"),
        input_tokens: 30,
        output_tokens: 7,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        extra_total_tokens: 0,
        cost: 0.03,
        credits: None,
        message_count: Some(1),
        version: Some("1.0.0"),
        missing_pricing_model: None,
    }));

    let row = accumulator.into_summary().unwrap();

    assert_eq!(row.models_used, vec!["gpt-5.5"]);
    assert_eq!(row.model_breakdowns.len(), 1);
    assert_eq!(row.model_breakdowns[0].model_name, "gpt-5.5");
    assert_eq!(row.model_breakdowns[0].input_tokens, 50);
    assert_eq!(row.model_breakdowns[0].output_tokens, 12);
    assert_eq!(row.model_breakdowns[0].cost, 0.05);
}

struct LoadedEntryFixture {
    date: &'static str,
    timestamp: i64,
    session_id: &'static str,
    project_path: &'static str,
    model: Option<&'static str>,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    extra_total_tokens: u64,
    cost: f64,
    credits: Option<f64>,
    message_count: Option<u64>,
    version: Option<&'static str>,
    missing_pricing_model: Option<&'static str>,
}

fn loaded_entry(fixture: LoadedEntryFixture) -> LoadedEntry {
    let usage = TokenUsageRaw {
        input_tokens: fixture.input_tokens,
        output_tokens: fixture.output_tokens,
        cache_creation_input_tokens: fixture.cache_creation_tokens,
        cache_read_input_tokens: fixture.cache_read_tokens,
        speed: None,
        cache_creation: None,
    };
    let timestamp = TimestampMs::from_millis(fixture.timestamp);
    LoadedEntry {
        usage,
        message_id: Some(format!("msg-{}", fixture.timestamp)),
        version: fixture.version.map(str::to_string),
        recorded_cost: None,
        timestamp,
        date: crate::cli::IsoDate::parse(fixture.date).unwrap(),
        project: Arc::from(fixture.project_path),
        session_id: Arc::from(fixture.session_id),
        project_path: Arc::from(fixture.project_path),
        cost: fixture.cost,
        extra_total_tokens: fixture.extra_total_tokens,
        credits: fixture.credits,
        message_count: fixture.message_count,
        model: fixture.model.map(str::to_string),
        usage_limit_reset_time: None,
        missing_pricing_model: fixture.missing_pricing_model.map(str::to_string),
        stored_cost: None,
        estimated_pricing: false,
    }
}

struct SummaryFixture {
    date: Option<&'static str>,
    model: &'static str,
    cost: f64,
    input_tokens: u64,
}

fn summary_row(fixture: SummaryFixture) -> UsageSummary {
    UsageSummary {
        date: fixture.date.map(str::to_string),
        month: None,
        week: None,
        session_id: None,
        project_path: None,
        last_activity: None,
        first_activity: None,
        input_tokens: fixture.input_tokens,
        output_tokens: 10,
        cache_creation_tokens: 1,
        cache_read_tokens: 2,
        extra_total_tokens: 3,
        total_cost: fixture.cost,
        credits: Some(0.5),
        message_count: Some(1),
        models_used: vec![fixture.model.to_string()],
        model_breakdowns: vec![ModelBreakdown {
            model_name: fixture.model.to_string(),
            input_tokens: fixture.input_tokens,
            output_tokens: 10,
            cache_creation_tokens: 1,
            cache_read_tokens: 2,
            cache_creation_1h_tokens: None,
            extra_total_tokens: 3,
            cost: fixture.cost,
            missing_pricing: false,
            stored_cost: None,
            estimated_pricing: false,
        }],
        project: None,
        versions: None,
    }
}
