use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use jiff::tz::TimeZone as JiffTimeZone;

use super::{
    parser::{DevinStep, calculate_devin_cost, load_transcript_file, missing_devin_pricing},
    paths::paths,
};
use crate::{
    LoadedEntry, PricingMap, Result, UsageEntry, UsageMessage, cli::SharedArgs,
    collect_files_with_extension, debug_log, format_date_tz, parse_tz, read_files_parallel,
};

pub fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent("Devin"),
        shared.json,
        || load_entries_inner(shared, pricing),
    )
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let mut files = transcript_files()?;
    files.sort();
    let loaded = read_files_parallel(&files, shared.single_thread, |file| {
        load_transcript_file(file).unwrap_or_else(|error| {
            debug_log(
                shared,
                format!(
                    "Failed to read Devin transcript {}: {error}",
                    file.display()
                ),
            );
            Vec::new()
        })
    });
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for steps in loaded {
        for step in steps {
            // Two configured roots can hold the same transcript (the legacy
            // cognition path is usually a symlink), so dedupe on the step.
            if !seen.insert((step.session_id.clone(), step.step_key.clone())) {
                continue;
            }
            entries.push(to_loaded_entry(step, tz.as_ref(), pricing));
        }
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

fn transcript_files() -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for root in paths()? {
        collect_files_with_extension(&root, "json", &mut files);
    }
    Ok(files)
}

pub(super) fn has_source(path: &Path) -> bool {
    has_json_file(path)
}

// Mirrors `collect_files_with_extension` but stops at the first match:
// detection should stay cheap next to a large transcripts dump.
fn has_json_file(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.filter_map(std::result::Result::ok).any(|entry| {
        let Ok(file_type) = entry.file_type() else {
            return false;
        };
        let path = entry.path();
        if file_type.is_file() {
            path.extension()
                .is_some_and(|extension| extension == "json")
        } else {
            file_type.is_dir() && has_json_file(&path)
        }
    })
}

fn to_loaded_entry(
    step: DevinStep,
    tz: Option<&JiffTimeZone>,
    pricing: &PricingMap,
) -> LoadedEntry {
    let cost = calculate_devin_cost(&step, pricing);
    let missing_pricing_model = missing_devin_pricing(&step, pricing);
    let data = UsageEntry {
        session_id: Some(step.session_id.clone()),
        timestamp: step.timestamp_text.clone(),
        version: step.version.clone(),
        message: UsageMessage {
            usage: step.usage,
            model: Some(step.model.clone()),
            id: Some(format!("devin:{}:{}", step.session_id, step.step_key)),
        },
        cost_usd: None,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };
    LoadedEntry {
        date: format_date_tz(step.timestamp, tz),
        timestamp: step.timestamp,
        project: Arc::from("devin"),
        session_id: Arc::from(step.session_id.as_str()),
        project_path: Arc::from("Devin"),
        cost,
        credits: None,
        extra_total_tokens: 0,
        model: Some(step.model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        message_count: None,
        data,
    }
}

#[cfg(test)]
mod tests {
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    use super::super::paths::DEVIN_TRANSCRIPTS_DIR_ENV;
    use super::*;
    use crate::{TokenUsageRaw, UsageEntry, UsageMessage, cli::AgentReportKind};

    const TRANSCRIPT: &str = r#"{
        "schema_version": "ATIF-v1.7",
        "session_id": "veil-vibraphone",
        "agent": {"name": "devin", "version": "3000.6.12", "model_name": "SWE-2 High"},
        "steps": [
            {"step_id": 1, "timestamp": "2026-09-10T16:51:21.988374+00:00", "source": "system", "message": "You are Devin."},
            {"step_id": 7, "timestamp": "2026-09-10T17:00:00.000000+00:00", "source": "agent", "model_name": "SWE-2 High",
             "metrics": {"prompt_tokens": 160245, "completion_tokens": 84},
             "extra": {"generation_model": "swe-2-high"}},
            {"step_id": 8, "timestamp": "2026-09-11T03:21:00.807570+00:00", "source": "agent", "model_name": "SWE-2 High",
             "metrics": {"prompt_tokens": 162395, "completion_tokens": 84, "cached_tokens": 160244,
                         "extra": {"cache_creation_input_tokens": 0}},
             "extra": {"generation_model": "swe-2-high"}}
        ],
        "final_metrics": {"total_prompt_tokens": 322640, "total_completion_tokens": 168}
    }"#;

    #[test]
    fn loads_usage_entries_from_transcript_files() {
        let fixture = fs_fixture!({
            "transcripts/veil-vibraphone.json": TRANSCRIPT,
        });
        let _env = EnvVarGuard::set(
            DEVIN_TRANSCRIPTS_DIR_ENV,
            fixture.path("transcripts").into_os_string(),
        );
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        let entries = load_entries(&shared, &PricingMap::default()).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].session_id.as_ref(), "veil-vibraphone");
        assert_eq!(entries[0].date, "2026-09-10");
        assert_eq!(entries[1].date, "2026-09-11");
        assert_eq!(entries[0].model.as_deref(), Some("swe-2-high"));
        assert_eq!(entries[0].data.version.as_deref(), Some("3000.6.12"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 160245);
        assert_eq!(entries[0].data.message.usage.output_tokens, 84);
        assert_eq!(entries[1].data.message.usage.input_tokens, 2151);
        assert_eq!(
            entries[1].data.message.usage.cache_read_input_tokens,
            160244
        );
    }

    #[test]
    fn dedupes_the_same_transcript_reachable_from_two_roots() {
        let fixture = fs_fixture!({
            "a/veil-vibraphone.json": TRANSCRIPT,
            "b/veil-vibraphone.json": TRANSCRIPT,
        });
        let raw = format!(
            "{},{}",
            fixture.path("a").display(),
            fixture.path("b").display()
        );
        let _env = EnvVarGuard::set(DEVIN_TRANSCRIPTS_DIR_ENV, raw);

        let entries = load_entries(&SharedArgs::default(), &PricingMap::default()).unwrap();

        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn loads_legacy_metadata_metrics() {
        let fixture = fs_fixture!({
            "transcripts/tasty-centaur.json": r#"{
                "schema_version": "ATIF-v1.4",
                "session_id": "tasty-centaur",
                "agent": {"name": "devin", "version": "2026.5.26-8", "model_name": "Adaptive"},
                "steps": [
                    {"step_id": 8, "source": "agent", "model_name": "Claude Opus 4.7",
                     "metadata": {
                        "created_at": "2026-06-10T13:01:55.298013Z",
                        "generation_model": "claude-opus-4-7-medium",
                        "metrics": {"input_tokens": 6, "output_tokens": 164,
                                    "cache_read_tokens": 14473, "cache_creation_tokens": 25474}
                     }},
                    {"step_id": 9, "source": "assistant",
                     "metadata": {
                        "created_at": "2026-06-10T13:05:00.000000Z",
                        "metrics": {"input_tokens": 3, "output_tokens": 10,
                                    "cache_read_tokens": 100, "cache_creation_tokens": 0}
                     }}
                ]
            }"#,
        });
        let _env = EnvVarGuard::set(
            DEVIN_TRANSCRIPTS_DIR_ENV,
            fixture.path("transcripts").into_os_string(),
        );

        let entries = load_entries(&SharedArgs::default(), &PricingMap::default()).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].data.message.usage.input_tokens, 6);
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 14473);
        assert_eq!(
            entries[0].data.message.usage.cache_creation_input_tokens,
            25474
        );
        assert_eq!(entries[0].model.as_deref(), Some("claude-opus-4-7-medium"));
    }

    #[test]
    fn skips_transcripts_without_metrics() {
        let fixture = fs_fixture!({
            "transcripts/legacy.json": r#"{
                "schema_version": "ATIF-v1.4",
                "session_id": "legacy",
                "steps": [{"step_id": 1, "source": "agent", "message": "hi"}]
            }"#,
            "transcripts/not-a-transcript.json": r#"{"unrelated": true}"#,
        });
        let _env = EnvVarGuard::set(
            DEVIN_TRANSCRIPTS_DIR_ENV,
            fixture.path("transcripts").into_os_string(),
        );

        let entries = load_entries(&SharedArgs::default(), &PricingMap::default()).unwrap();

        assert!(entries.is_empty());
    }

    #[test]
    fn falls_back_to_step_model_name_and_agent_model_name() {
        let fixture = fs_fixture!({
            "transcripts/display-name.json": r#"{
                "schema_version": "ATIF-v1.7",
                "session_id": "display-name",
                "agent": {"name": "devin", "version": "1.0.0", "model_name": "SWE-2 High"},
                "steps": [
                    {"step_id": 1, "timestamp": "2026-09-10T17:00:00Z", "source": "agent",
                     "model_name": "SWE-1.7 Lightning",
                     "metrics": {"prompt_tokens": 10, "completion_tokens": 5}}
                ]
            }"#,
            "transcripts/agent-level.json": r#"{
                "schema_version": "ATIF-v1.7",
                "session_id": "agent-level",
                "agent": {"name": "devin", "version": "1.0.0", "model_name": "SWE-2 High"},
                "steps": [
                    {"step_id": 1, "timestamp": "2026-09-10T17:00:00Z", "source": "agent",
                     "metrics": {"prompt_tokens": 10, "completion_tokens": 5}}
                ]
            }"#,
        });
        let _env = EnvVarGuard::set(
            DEVIN_TRANSCRIPTS_DIR_ENV,
            fixture.path("transcripts").into_os_string(),
        );

        let entries = load_entries(&SharedArgs::default(), &PricingMap::default()).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].model.as_deref(), Some("swe-2-high"));
        assert_eq!(entries[1].model.as_deref(), Some("swe-1.7-lightning"));
    }

    #[test]
    fn groups_steps_into_one_session_row() {
        let entries = vec![
            loaded_step("session-a", "1", "2026-09-10", 100, 10),
            loaded_step("session-a", "2", "2026-09-11", 200, 20),
            loaded_step("session-b", "1", "2026-09-11", 50, 5),
        ];

        let sessions =
            super::super::report::summarize_entries(&entries, AgentReportKind::Session).unwrap();
        let daily =
            super::super::report::summarize_entries(&entries, AgentReportKind::Daily).unwrap();

        assert_eq!(sessions.len(), 2);
        assert_eq!(daily.len(), 2);
        assert_eq!(sessions[0].session_id.as_deref(), Some("session-a"));
        assert_eq!(sessions[0].input_tokens, 300);
        assert_eq!(sessions[0].output_tokens, 30);
    }

    fn loaded_step(
        session_id: &str,
        step_key: &str,
        date: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> LoadedEntry {
        let timestamp = crate::parse_ts_timestamp(&format!("{date}T00:00:00.000Z")).unwrap();
        LoadedEntry {
            data: UsageEntry {
                session_id: Some(session_id.to_string()),
                timestamp: format!("{date}T00:00:00.000Z"),
                version: Some("1.0.0".to_string()),
                message: UsageMessage {
                    usage: TokenUsageRaw {
                        input_tokens,
                        output_tokens,
                        ..TokenUsageRaw::default()
                    },
                    model: Some("swe-2-high".to_string()),
                    id: Some(format!("devin:{session_id}:{step_key}")),
                },
                cost_usd: None,
                request_id: None,
                is_api_error_message: None,
                is_sidechain: None,
            },
            timestamp,
            date: date.to_string(),
            project: Arc::from("devin"),
            session_id: Arc::from(session_id),
            project_path: Arc::from("Devin"),
            cost: 0.0,
            credits: None,
            extra_total_tokens: 0,
            model: Some("swe-2-high".to_string()),
            usage_limit_reset_time: None,
            missing_pricing_model: None,
            message_count: None,
        }
    }
}
