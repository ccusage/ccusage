use std::{
    ffi::OsString,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use std::collections::BTreeMap;

use super::*;
use crate::{
    ColumnKind, ModelBreakdown, PricingMap,
    cli::{AgentReportKind, CostMode, SharedArgs, SourceArgs, WeekDay},
    loader::{aggregate_rows, load_agent_rows_parallel, load_rows, load_sections},
    report::{
        all_report_title, all_table_columns, all_table_row, report_json_with_agents,
        sections_report_json, source_table_row,
    },
    types::{AgentLoadSpec, AgentRows, Bucket},
    types::{DayRow, PeriodRow, ReportDay, ReportRow},
};
use ccusage_adapter_codex::{CodexGroup, CodexModelUsage};
use ccusage_adapter_common::{
    filter_loaded_entries_by_date,
    rows::{RowUsage, SourceRow},
};
use ccusage_core::model_aliases::set_model_aliases_for_tests;
use ccusage_test_support::{Environment, fs_fixture, instant, set_modified};

mod aggregation;
mod breakdowns;
mod claude_agent;
mod diagnostics;
mod fixtures;
mod json;

pub(crate) static NO_SOURCES: BTreeMap<String, SourceArgs> = BTreeMap::new();

pub(crate) fn aggregate_periods(rows: Vec<SourceRow>, bucket: Bucket) -> Option<Vec<ReportRow>> {
    rows.into_iter()
        .map(|row| ReportDay::parse(&row.period).map(|day| DayRow { day, row }))
        .collect::<Option<Vec<_>>>()
        .map(|rows| aggregate_rows(rows, bucket))
}

fn claude_usage_line(session: &str, timestamp: &str, message_id: &str, input: u64) -> String {
    format!(
        r#"{{"timestamp":"{timestamp}","sessionId":"{session}","requestId":"req-{message_id}","costUSD":1.0,"message":{{"id":"{message_id}","model":"claude-opus-4-20250514","usage":{{"input_tokens":{input},"output_tokens":70}}}}}}"#
    )
}

fn api_row(created: i64, input: u64) -> String {
    format!(
        r#"{{"time":{{"created":{created}}},"model":{{"id":"claude-opus-4-20250514","providerID":"anthropic"}},"content":[],"tokens":{{"input":{input},"output":5}},"cost":0.5}}"#
    )
}

fn write_opencode_rows(db_path: &Path, rows: &[(&str, &str, i64, String)]) {
    let db = sqlite::open(db_path).unwrap();
    db.execute(
        "CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT, seq INTEGER, time_created INTEGER, data TEXT)",
    )
    .unwrap();
    for (id, session, created, data) in rows {
        let mut statement = db
            .prepare("INSERT INTO session_message VALUES (?1, ?2, 'assistant', 1, ?3, ?4)")
            .unwrap();
        statement.bind((1, *id)).unwrap();
        statement.bind((2, *session)).unwrap();
        statement.bind((3, *created)).unwrap();
        statement.bind((4, data.as_str())).unwrap();
        statement.next().unwrap();
    }
}

fn claude_opencode_env(
    fixture: &ccusage_test_support::Fixture,
    claude_config_dir: OsString,
) -> Environment {
    isolated_agents_env(
        fixture,
        [
            (
                "OPENCODE_DATA_DIR",
                fixture.path("opencode").into_os_string(),
            ),
            ("CLAUDE_CONFIG_DIR", claude_config_dir),
        ],
    )
}

fn fixture_shared(since: &str, until: &str) -> SharedArgs {
    SharedArgs {
        since: Some(since.parse().unwrap()),
        until: Some(until.parse().unwrap()),
        timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
        offline: true,
        single_thread: true,
        ..SharedArgs::default()
    }
}

fn table_snapshot(
    rows: &[ReportRow],
    kind: AgentReportKind,
    detected_agents: &[&'static str],
) -> Value {
    let (headers, _) = all_table_columns(kind, false, false);
    let mut rendered_rows = Vec::new();
    for row in rows {
        rendered_rows.push(json!({
            "kind": "row",
            "cells": all_table_row(row, false, false, false),
        }));
        if let Some(agent_breakdowns) = row.agent_breakdowns() {
            for breakdown in agent_breakdowns {
                rendered_rows.push(json!({
                    "kind": "agent-breakdown",
                    "cells": source_table_row(breakdown, false, true, false),
                }));
            }
        }
    }
    let totals = report_json_with_agents(rows, kind, false)["totals"].clone();
    let total_row = ReportRow::Period(PeriodRow {
        usage: RowUsage {
            period: "Total".to_string(),
            models_used: Vec::new(),
            input_tokens: totals["inputTokens"].as_u64().unwrap(),
            output_tokens: totals["outputTokens"].as_u64().unwrap(),
            cache_creation_tokens: totals["cacheCreationTokens"].as_u64().unwrap(),
            cache_read_tokens: totals["cacheReadTokens"].as_u64().unwrap(),
            total_tokens: totals["totalTokens"].as_u64().unwrap(),
            total_cost: totals["totalCost"].as_f64().unwrap(),
            model_breakdowns: Vec::new(),
        },
        agents: Vec::new(),
        agent_breakdowns: Vec::new(),
    });
    let mut total_cells = all_table_row(&total_row, false, false, false);
    total_cells[1].clear();
    total_cells[2].clear();
    rendered_rows.push(json!({
        "kind": "total",
        "cells": total_cells,
    }));

    json!({
        "title": all_report_title(kind, detected_agents),
        "headers": headers,
        "rows": rendered_rows,
    })
}

fn isolated_agent_env(
    fixture: &ccusage_test_support::Fixture,
    source_key: &'static str,
    source_value: OsString,
) -> Environment {
    isolated_agents_env(fixture, [(source_key, source_value)])
}

fn isolated_agents_env(
    fixture: &ccusage_test_support::Fixture,
    sources: impl IntoIterator<Item = (&'static str, OsString)>,
) -> Environment {
    fixture.create_dir_all("empty-xdg-config");
    fixture.create_dir_all("empty-xdg-data");
    let home = fixture.path("empty-home").into_os_string();
    let xdg_config = fixture.path("empty-xdg-config").into_os_string();
    let xdg_data = fixture.path("empty-xdg-data").into_os_string();
    let mut vars = [
        "CLAUDE_CONFIG_DIR",
        "CODEX_HOME",
        "OPENCODE_DATA_DIR",
        "AMP_DATA_DIR",
        "DROID_SESSIONS_DIR",
        "CODEBUFF_DATA_DIR",
        "HERMES_HOME",
        "PI_AGENT_DIR",
        "GOOSE_PATH_ROOT",
        "OPENCLAW_DIR",
        "KILO_DATA_DIR",
        "COPILOT_OTEL_FILE_EXPORTER_PATH",
        "GEMINI_DATA_DIR",
        "KIMI_DATA_DIR",
        "QWEN_DATA_DIR",
        "GROK_HOME",
        "ZCODE_HOME",
        "ANTIGRAVITY_DATA_DIR",
        "CLINE_DATA_DIR",
        "DSH_HOME",
        "APPDATA",
        "COPILOT_HOME",
        "LOG_LEVEL",
    ]
    .into_iter()
    .map(|key| (key, None::<OsString>))
    .collect::<Vec<_>>();
    vars.push(("HOME", Some(home)));
    vars.push((
        "USERPROFILE",
        Some(fixture.path("empty-userprofile").into_os_string()),
    ));
    vars.push(("XDG_CONFIG_HOME", Some(xdg_config)));
    vars.push(("XDG_DATA_HOME", Some(xdg_data)));
    vars.extend(sources.into_iter().map(|(key, value)| (key, Some(value))));
    Environment::with_vars(vars)
}

fn assert_daily_family_and_session_sections_match_standalone(shared: &SharedArgs) {
    for command_kind in [AgentReportKind::Daily, AgentReportKind::Session] {
        let sections = match command_kind {
            AgentReportKind::Daily => vec![
                AgentReportKind::Daily,
                AgentReportKind::Weekly,
                AgentReportKind::Monthly,
                AgentReportKind::Session,
            ],
            AgentReportKind::Session => vec![
                AgentReportKind::Session,
                AgentReportKind::Daily,
                AgentReportKind::Weekly,
                AgentReportKind::Monthly,
            ],
            AgentReportKind::Weekly | AgentReportKind::Monthly => unreachable!(),
        };
        let section_rows = load_sections(&sections, shared, &NO_SOURCES, WeekDay::Monday).unwrap();
        let report = serde_json::to_value(sections_report_json(
            &section_rows.sections,
            command_kind,
            false,
        ))
        .unwrap();

        for section_kind in [
            AgentReportKind::Daily,
            AgentReportKind::Weekly,
            AgentReportKind::Monthly,
            AgentReportKind::Session,
        ] {
            let standalone = load_rows(section_kind, shared, &NO_SOURCES, WeekDay::Monday).unwrap();
            let standalone_report = report_json_with_agents(&standalone.rows, section_kind, false);
            let key = match section_kind {
                AgentReportKind::Daily => "daily",
                AgentReportKind::Weekly => "weekly",
                AgentReportKind::Monthly => "monthly",
                AgentReportKind::Session => "session",
            };
            assert_eq!(
                report[key], standalone_report[key],
                "{command_kind:?} invocation should match standalone {section_kind:?}"
            );
        }
    }
}
