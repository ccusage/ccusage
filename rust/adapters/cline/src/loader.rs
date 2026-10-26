use std::{collections::HashMap, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;

use crate::{LoadedEntry, PricingMap, Result, cli::SharedArgs, debug_log, parse_tz};
use ccusage_adapter_common::read_files_parallel;

use super::paths::discover_files;

pub(super) struct EntryInput {
    pub timestamp: crate::TimestampMs,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub usage: crate::TokenUsageRaw,
    pub reported_cost: Option<f64>,
    pub request_id: Option<String>,
}

pub fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent("Cline"),
        shared.json,
        || load_entries_inner(shared, pricing),
    )
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let sources = discover_files()?;
    let timezone = parse_tz(shared.timezone.as_deref());
    let files = sources
        .iter()
        .map(|source| source.path.clone())
        .collect::<Vec<_>>();
    let source_by_path = sources
        .iter()
        .map(|source| (source.path.as_path(), source))
        .collect::<HashMap<_, _>>();
    let mut entries = read_files_parallel(&files, shared.single_thread, |path| {
        let source = source_by_path
            .get(path)
            .copied()
            .expect("discovered Cline file has source metadata");
        super::parser::read_file(source, shared.mode, pricing, timezone.as_ref()).unwrap_or_else(
            |error| {
                debug_log(
                    shared,
                    format!(
                        "Failed to read Cline source file {}: {error}",
                        path.display()
                    ),
                );
                Vec::new()
            },
        )
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

pub(super) fn loaded_entry(
    source: &super::paths::SourceFile,
    input: EntryInput,
    mode: crate::cli::CostMode,
    pricing: &PricingMap,
    timezone: Option<&JiffTimeZone>,
) -> LoadedEntry {
    use crate::{
        UsageEntry, UsageMessage, calculate_cost_for_usage_at, format_date_tz,
        missing_pricing_model_for_candidates, stored_cost_for_mode, total_usage_tokens,
    };

    let EntryInput {
        timestamp,
        model,
        provider,
        usage,
        reported_cost,
        request_id,
    } = input;
    let timestamp_text = crate::format_rfc3339_millis(timestamp);
    let session_id = match source.kind {
        super::paths::SourceKind::Extension | super::paths::SourceKind::CliTask => {
            format!("task:{}", source.session_id)
        }
        super::paths::SourceKind::Cli => format!("cli:{}", source.session_id),
    };
    let provider_model = model
        .as_deref()
        .zip(provider.as_deref())
        .filter(|(model, _)| !model.contains('/'))
        .map(|(model, provider)| format!("{provider}/{model}"));
    let model_for_pricing = model
        .as_deref()
        .filter(|model| pricing.find(model).is_some())
        .or_else(|| {
            provider_model
                .as_deref()
                .filter(|model| pricing.find(model).is_some())
        })
        .or(model.as_deref());
    let cost = calculate_cost_for_usage_at(
        model_for_pricing,
        usage,
        reported_cost,
        Some(timestamp),
        mode,
        Some(pricing),
    );
    let missing_pricing_model = if mode == crate::cli::CostMode::Display
        || (mode == crate::cli::CostMode::Auto && reported_cost.is_some())
        || total_usage_tokens(usage) == 0
    {
        None
    } else if let Some(model) = model.as_deref() {
        let candidates = std::iter::once(model.to_string()).chain(provider_model);
        missing_pricing_model_for_candidates(
            model,
            candidates,
            total_usage_tokens(usage),
            Some(pricing),
        )
    } else {
        Some("unknown".to_string())
    };
    let display_model = model.clone().unwrap_or_else(|| "unknown".to_string());
    let data = UsageEntry {
        session_id: Some(session_id.clone()),
        timestamp: timestamp_text,
        version: None,
        message: UsageMessage {
            usage,
            model: model.clone(),
            id: request_id.clone(),
        },
        cost_usd: reported_cost,
        request_id,
        is_api_error_message: None,
        is_sidechain: None,
    };

    LoadedEntry {
        date: format_date_tz(timestamp, timezone),
        timestamp,
        project: Arc::from("cline"),
        session_id: Arc::from(session_id),
        project_path: Arc::from("Cline"),
        cost,
        credits: None,
        model: Some(display_model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        extra_total_tokens: 0,
        message_count: None,
        data,
        stored_cost: stored_cost_for_mode(reported_cost, mode),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::CostMode;
    use ccusage_test_support::{EnvVarGuard, Fixture};

    #[test]
    fn loads_extension_and_cli_fixture_records_with_source_namespaced_sessions() {
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
        let _env = EnvVarGuard::set(
            super::super::paths::CLINE_DATA_DIR_ENV,
            format!(
                "{},{}",
                fixture.root().display(),
                fixture.root().join("data").display()
            ),
        );
        let shared = SharedArgs {
            json: true,
            single_thread: true,
            timezone: Some("UTC".to_string()),
            mode: CostMode::Auto,
            ..SharedArgs::default()
        };

        let entries = load_entries(&shared, &PricingMap::load_embedded()).unwrap();

        assert_eq!(
            entries.len(),
            6,
            "partial API requests are not usage records"
        );
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.date.as_str())
                .collect::<Vec<_>>(),
            [
                "2026-10-01",
                "2026-10-31",
                "2026-11-01",
                "2026-11-01",
                "2026-11-01",
                "2026-11-01"
            ]
        );
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.session_id.as_ref())
                .collect::<Vec<_>>(),
            [
                "task:session-a",
                "task:session-a",
                "cli:session-a",
                "cli:session-a",
                "task:session-a",
                "task:session-a"
            ]
        );
        assert_eq!(
            entries[0].model.as_deref(),
            Some("claude-sonnet-4-20250514")
        );
        assert_eq!(entries[1].model.as_deref(), Some("claude-opus-4-20250514"));
        assert_eq!(entries[1].data.message.usage.cache_read_input_tokens, 20);
        assert_eq!(
            entries[1].data.message.usage.cache_creation_input_tokens,
            30
        );
        assert_eq!(entries[2].data.message.id.as_deref(), Some("assistant-1"));
        assert_eq!(entries[2].data.message.usage.input_tokens, 65);
        assert_eq!(entries[2].data.cost_usd, Some(0.0));
        assert_eq!(entries[2].stored_cost, Some(0.0));
        assert_eq!(entries[3].data.message.id.as_deref(), Some("assistant-2"));
        assert_eq!(entries[3].data.message.usage.input_tokens, 90);
        assert_eq!(entries[3].data.message.usage.output_tokens, 16);
        assert_eq!(entries[3].data.cost_usd, Some(0.0002));
        assert_eq!(entries[4].data.message.usage.input_tokens, 8);
        assert_eq!(entries[4].data.message.usage.output_tokens, 4);
        assert_eq!(entries[4].model.as_deref(), Some("unknown"));
        assert_eq!(entries[4].missing_pricing_model.as_deref(), Some("unknown"));
        assert_eq!(entries[5].model.as_deref(), Some("unknown"));
        assert_eq!(entries[5].missing_pricing_model.as_deref(), Some("unknown"));
    }
}
