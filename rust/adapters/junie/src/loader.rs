use std::{collections::HashMap, path::Path, sync::Arc};

use ccusage_adapter_common::read_files_parallel;
use jiff::tz::TimeZone as JiffTimeZone;

use super::{
    parser::{JunieUsage, SessionMeta, parse_events, parse_session_index},
    paths::{discover_events_files, session_index_path},
};
use crate::{
    LoadedEntry, PricingMap, Result, UsageEntry, UsageMessage, calculate_cost_for_usage_at,
    cli::{CostMode, SharedArgs},
    debug_log, format_date_tz, format_rfc3339_millis, missing_pricing_model_for_usage, parse_tz,
    stored_cost_for_mode,
};

pub fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent("Junie"),
        shared.json,
        || load_entries_inner(shared, pricing),
    )
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let files = discover_events_files(shared);
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let index = load_session_index(shared);
    let timezone = parse_tz(shared.timezone.as_deref());
    let mut entries = read_files_parallel(&files, shared.single_thread, |path| {
        let session_id = session_id_for(path);
        let meta = index.get(session_id);
        let content = match std::fs::read(path) {
            Ok(content) => content,
            Err(error) => {
                debug_log(
                    shared,
                    format!(
                        "Failed to read Junie events file {}: {error}",
                        path.display()
                    ),
                );
                return Vec::new();
            }
        };
        parse_events(&content, meta.and_then(|meta| meta.created_at))
            .into_iter()
            .map(|usage| {
                loaded_entry(
                    session_id,
                    meta,
                    usage,
                    shared.mode,
                    pricing,
                    timezone.as_ref(),
                )
            })
            .collect::<Vec<_>>()
    })
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        left.timestamp
            .cmp(&right.timestamp)
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
    Ok(entries)
}

fn load_session_index(shared: &SharedArgs) -> HashMap<String, SessionMeta> {
    let Some(path) = session_index_path() else {
        return HashMap::new();
    };
    match std::fs::read(&path) {
        Ok(content) => parse_session_index(&content),
        Err(error) => {
            debug_log(
                shared,
                format!(
                    "Failed to read Junie session index {}: {error}",
                    path.display()
                ),
            );
            HashMap::new()
        }
    }
}

fn session_id_for(events_file: &Path) -> &str {
    events_file
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or("unknown")
}

fn loaded_entry(
    session_id: &str,
    meta: Option<&SessionMeta>,
    record: JunieUsage,
    mode: CostMode,
    pricing: &PricingMap,
    timezone: Option<&JiffTimeZone>,
) -> LoadedEntry {
    let JunieUsage {
        timestamp,
        model,
        usage,
        cost: reported_cost,
    } = record;
    let cost = calculate_cost_for_usage_at(
        Some(&model),
        usage,
        reported_cost,
        Some(timestamp),
        mode,
        Some(pricing),
    );
    let missing_pricing_model =
        missing_pricing_model_for_usage(Some(&model), usage, reported_cost, mode, Some(pricing));
    let project_path = meta
        .and_then(|meta| meta.project_dir.as_deref())
        .unwrap_or("Junie");
    LoadedEntry {
        date: format_date_tz(timestamp, timezone),
        timestamp,
        project: Arc::from("junie"),
        session_id: Arc::from(session_id),
        project_path: Arc::from(project_path),
        cost,
        extra_total_tokens: 0,
        credits: None,
        message_count: None,
        model: Some(model.clone()),
        data: UsageEntry {
            session_id: Some(session_id.to_string()),
            timestamp: format_rfc3339_millis(timestamp),
            version: None,
            message: UsageMessage {
                usage,
                model: Some(model),
                id: None,
            },
            cost_usd: reported_cost,
            request_id: None,
            is_api_error_message: None,
            is_sidechain: None,
        },
        usage_limit_reset_time: None,
        missing_pricing_model,
        stored_cost: stored_cost_for_mode(reported_cost, mode),
    }
}

#[cfg(test)]
mod tests {
    use super::super::paths::JUNIE_HOME_ENV;
    use super::*;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    const EVENT: &str = r#"{"kind":"SessionA2uxEvent","event":{"agentEvent":{"kind":"LlmResponseMetadataEvent","modelUsage":[{"model":"gpt-4.1-2025-04-14","cost":0.00225,"inputTokens":1121,"outputTokens":1}]}},"timestampMs":1785470253773}"#;
    const LEGACY_EVENT: &str = r#"{"kind":"SessionA2uxEvent","event":{"agentEvent":{"kind":"LlmResponseMetadataEvent","modelUsage":[{"model":"gpt-4.1-2025-04-14","cost":0.00225,"inputTokens":1121,"outputTokens":1}]}}}"#;

    fn utc_shared(mode: CostMode) -> SharedArgs {
        SharedArgs {
            timezone: Some("UTC".to_string()),
            mode,
            ..SharedArgs::default()
        }
    }

    #[test]
    fn attaches_index_metadata_to_each_session() {
        let fixture = fs_fixture!({
            "sessions/index.jsonl": [
                r#"{"sessionId":"session-a","createdAt":1779231600000,"projectDir":"/work/app"}"#,
            ]
            .join("\n"),
            "sessions/session-a/events.jsonl": [EVENT, LEGACY_EVENT].join("\n"),
            "sessions/session-b/events.jsonl": EVENT,
        });
        let _home = EnvVarGuard::set(JUNIE_HOME_ENV, fixture.root());

        let entries =
            load_entries(&utc_shared(CostMode::Auto), &PricingMap::load_embedded()).unwrap();

        let rows = entries
            .iter()
            .map(|entry| {
                (
                    entry.date.as_str(),
                    entry.session_id.as_ref(),
                    entry.project.as_ref(),
                    entry.project_path.as_ref(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            rows,
            [
                ("2026-05-19", "session-a", "junie", "/work/app"),
                ("2026-07-31", "session-a", "junie", "/work/app"),
                ("2026-07-31", "session-b", "junie", "Junie"),
            ]
        );
    }

    #[test]
    fn loads_events_without_an_index() {
        let fixture = fs_fixture!({
            "sessions/session-a/events.jsonl": [EVENT, LEGACY_EVENT].join("\n"),
        });
        let _home = EnvVarGuard::set(JUNIE_HOME_ENV, fixture.root());

        let entries =
            load_entries(&utc_shared(CostMode::Auto), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2026-07-31");
    }

    #[test]
    fn cost_modes_choose_between_recorded_and_calculated_cost() {
        let fixture = fs_fixture!({
            "sessions/session-a/events.jsonl": r#"{"event":{"agentEvent":{"kind":"LlmResponseMetadataEvent","modelUsage":[{"model":"gpt-4.1-2025-04-14","cost":0.5,"inputTokens":1121,"outputTokens":1}]}},"timestampMs":1785470253773}"#,
        });
        let _home = EnvVarGuard::set(JUNIE_HOME_ENV, fixture.root());
        let pricing = PricingMap::load_embedded();
        let cost = |mode| load_entries(&utc_shared(mode), &pricing).unwrap()[0].cost;

        assert_eq!(cost(CostMode::Auto), 0.5);
        assert_eq!(cost(CostMode::Display), 0.5);
        let calculated = cost(CostMode::Calculate);
        assert!(
            (calculated - 0.00225).abs() < 1e-9,
            "calculate mode prices gpt-4.1 tokens: {calculated}"
        );
    }

    #[test]
    fn flags_unpriced_models_without_a_recorded_cost() {
        let fixture = fs_fixture!({
            "sessions/session-a/events.jsonl": r#"{"event":{"agentEvent":{"kind":"LlmResponseMetadataEvent","modelUsage":[{"model":"vendor/unknown-model","inputTokens":10,"outputTokens":1}]}},"timestampMs":1785470253773}"#,
        });
        let _home = EnvVarGuard::set(JUNIE_HOME_ENV, fixture.root());

        let entries =
            load_entries(&utc_shared(CostMode::Auto), &PricingMap::load_embedded()).unwrap();

        assert_eq!(
            entries[0].missing_pricing_model.as_deref(),
            Some("vendor/unknown-model")
        );
    }
}
