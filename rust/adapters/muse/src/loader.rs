use std::{
    collections::HashSet,
    fs,
    path::{Component, Path},
};

use ccusage_core::{LoadedEntry, PricingMap, Result, cli::SharedArgs, debug_log, parse_tz};
use serde_json::Value;

use crate::{
    fast::LinePrefilter,
    jsonl,
    parser::{
        MuseUsageEntry, muse_entry_to_loaded, parse_metadata_record, parse_model_completed_value,
    },
    paths::discover_session_files,
    read_files_parallel,
};

pub fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent("Muse Code"),
        shared.json,
        || load_entries_inner(shared, pricing),
    )
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let files = discover_session_files()?;
    let loaded = read_files_parallel(&files, shared.single_thread, |file| {
        read_session_file(file).unwrap_or_else(|error| {
            debug_log(
                shared,
                format!(
                    "Failed to read Muse session file {}: {error}",
                    file.display()
                ),
            );
            Vec::new()
        })
    });

    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for (file, file_entries) in files.iter().zip(loaded) {
        let (session_id, session_scope) = session_path_info(file);
        for entry in file_entries {
            if seen.insert(record_key(&session_scope, &entry)) {
                entries.push(muse_entry_to_loaded(
                    entry,
                    &session_id,
                    tz.as_ref(),
                    shared.mode,
                    pricing,
                ));
            }
        }
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

fn read_session_file(path: &Path) -> Result<Vec<MuseUsageEntry>> {
    let content = fs::read(path)?;
    let prefilter =
        LinePrefilter::any(&[br#""model_completed""#, br#""runtime.session.metadata""#]);
    let mut entries = Vec::new();
    let mut current_metadata = None;
    for (record_index, value) in jsonl::records::<Value>(&content, Some(&prefilter)).enumerate() {
        if let Some(metadata) = parse_metadata_record(&value) {
            if current_metadata.is_none() {
                for entry in &mut entries {
                    metadata.apply_to(entry);
                }
            }
            current_metadata = Some(metadata);
            continue;
        }
        if let Some(mut entry) = parse_model_completed_value(&value, record_index) {
            if let Some(metadata) = &current_metadata {
                metadata.apply_to(&mut entry);
            }
            entries.push(entry);
        }
    }
    Ok(entries)
}

#[derive(Debug, Hash, PartialEq, Eq)]
enum RecordKey {
    Identified {
        session_scope: String,
        stream_id: String,
        record_id: String,
    },
    Fallback {
        session_scope: String,
        record_index: usize,
    },
}

fn record_key(session_scope: &str, entry: &MuseUsageEntry) -> RecordKey {
    match (&entry.stream_id, &entry.record_id) {
        (Some(stream_id), Some(record_id)) => RecordKey::Identified {
            session_scope: session_scope.to_owned(),
            stream_id: stream_id.clone(),
            record_id: record_id.clone(),
        },
        _ => RecordKey::Fallback {
            session_scope: session_scope.to_owned(),
            record_index: entry.line_index,
        },
    }
}

fn session_path_info(file: &Path) -> (String, String) {
    let parent = file.parent().unwrap_or_else(|| Path::new(""));
    let components = parent
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let sessions_index = components
        .iter()
        .rposition(|component| component == "sessions");
    let relative = sessions_index
        .map(|index| &components[index + 1..])
        .unwrap_or(&components);
    let session_id = relative
        .last()
        .or_else(|| components.last())
        .cloned()
        .unwrap_or_else(|| "unknown".to_owned());
    let session_scope = if relative.is_empty() {
        parent.to_string_lossy().into_owned()
    } else {
        relative.join("/")
    };
    (session_id, session_scope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    #[test]
    fn deduplicates_replayed_records_without_collapsing_distinct_calls_or_sessions() {
        let fixture = fs_fixture!({
            "muse/sessions/2026/09/07/session-a/session.jsonl": [
                completion_record("stream-a", "record-a", 1_000_000),
                completion_record("stream-a", "record-a", 1_000_000),
                completion_record("stream-a", "record-b", 1_000_000),
                r#"{"recorded_at":1000000,"stream":{"id":"stream-a"},"id":"partial","payload_type":"runtime.session","payload":{"event":{"kind":"model_streaming","usage":{"input_tokens":999,"output_tokens":999}}}}"#.to_owned(),
                r#"{"recorded_at":1000000,"stream":{"id":"stream-a"},"id":"total","payload_type":"runtime.session","payload":{"event":{"kind":"session_usage","usage":{"input_tokens":999,"output_tokens":999}}}}"#.to_owned(),
            ]
            .join("\n"),
            "muse/sessions/2026/09/07/session-b/session.jsonl":
                completion_record("stream-a", "record-a", 1_000_000),
        });
        let _cleanup = EnvVarGuard::set("MUSE_DATA_DIR", fixture.path("muse"));
        let entries =
            load_entries_inner(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].session_id.as_ref(), "session-a");
        assert_eq!(entries[1].session_id.as_ref(), "session-a");
        assert_eq!(entries[2].session_id.as_ref(), "session-b");
        assert!(
            entries
                .iter()
                .all(|entry| entry.data.message.usage.input_tokens == 8)
        );
        assert!(
            entries
                .iter()
                .all(|entry| entry.data.message.usage.output_tokens == 2)
        );
    }

    #[test]
    fn preserves_identical_records_when_durable_identity_is_missing() {
        let record = r#"{"recorded_at":1000000,"payload_type":"runtime.session","payload":{"event":{"kind":"model_completed","model":"muse-spark-1.2","usage":{"input_tokens":5,"output_tokens":2}}}}"#;
        let fixture = fs_fixture!({
            "muse/sessions/2026/09/07/session-a/session.jsonl": format!("{record}\n{record}\n"),
        });
        let _cleanup = EnvVarGuard::set("MUSE_DATA_DIR", fixture.path("muse"));
        let entries =
            load_entries_inner(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn ignores_unrelated_nested_values_in_metadata_envelopes() {
        let fixture = fs_fixture!({
            "muse/sessions/2026/09/07/session-a/session.jsonl": [
                r#"{"payload_type":"runtime.session.metadata","payload":{"context":{"model":"unrelated-model","provider":"unrelated-provider","cwd":"/unrelated/project"}}}"#.to_owned(),
                completion_without_model(1_000_000),
            ]
            .join("\n"),
            "muse/sessions/2026/09/07/session-b/session.jsonl":
                completion_without_model(2_000_000),
        });
        let _cleanup = EnvVarGuard::set("MUSE_DATA_DIR", fixture.path("muse"));
        let entries =
            load_entries_inner(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries[0].model.as_deref(), Some("unknown"));
        assert_eq!(entries[0].project_path.as_ref(), "Muse Code");
        assert_eq!(entries[1].model.as_deref(), Some("unknown"));
        assert_eq!(entries[1].project_path.as_ref(), "Muse Code");
    }

    #[test]
    fn uses_exact_metadata_snapshots_for_initial_backfill_and_forward_fallbacks() {
        let fixture = fs_fixture!({
            "muse/sessions/2026/09/07/session-a/session.jsonl": include_str!(
                "../fixtures/metadata-timeline-synthetic.jsonl"
            ),
        });
        let _cleanup = EnvVarGuard::set("MUSE_DATA_DIR", fixture.path("muse"));
        let mut pricing = PricingMap::default();
        assert_eq!(
            pricing.load_json(
                r#"{
                    "openrouter/muse-spark-1.2": { "input_cost_per_token": 0.1, "output_cost_per_token": 0 },
                    "muse-spark-1.2": { "input_cost_per_token": 0.2, "output_cost_per_token": 0 },
                    "meta/muse-spark-1.2-contributor": { "input_cost_per_token": 0.5, "output_cost_per_token": 0 }
                }"#
            ),
            3
        );
        let shared = SharedArgs {
            mode: ccusage_core::cli::CostMode::Calculate,
            ..SharedArgs::default()
        };
        let entries = load_entries_inner(&shared, &pricing).unwrap();

        assert_eq!(entries.len(), 5);
        assert_eq!(entries[0].model.as_deref(), Some("muse-spark-1.2"));
        assert_eq!(entries[0].project_path.as_ref(), "/workspace/initial");
        assert_eq!(entries[1].model.as_deref(), Some("muse-spark-1.2"));
        assert_eq!(entries[1].project_path.as_ref(), "/workspace/initial");
        assert_eq!(entries[2].model.as_deref(), Some("muse-spark-1.2"));
        assert_eq!(entries[2].project_path.as_ref(), "/workspace/switched");
        assert_eq!(
            entries[3].model.as_deref(),
            Some("muse-spark-1.2-contributor")
        );
        assert_eq!(entries[3].project_path.as_ref(), "/workspace/switched");
        assert_eq!(entries[4].model.as_deref(), Some("muse-spark-1.2"));
        assert_eq!(entries[4].project_path.as_ref(), "/workspace/reset");
        assert!((entries[0].cost - 0.4).abs() < 0.000_001);
        assert!((entries[1].cost - 0.4).abs() < 0.000_001);
        assert!((entries[2].cost - 0.8).abs() < 0.000_001);
        assert!((entries[3].cost - 2.0).abs() < 0.000_001);
        assert!((entries[4].cost - 0.8).abs() < 0.000_001);
    }

    #[test]
    fn subagent_files_keep_nearest_session_ids_and_dedupe_scopes() {
        let fixture = fs_fixture!({
            "muse/sessions/2026/09/07/session-parent/session.jsonl": include_str!(
                "../fixtures/subagent-parent-synthetic.jsonl"
            ),
            "muse/sessions/2026/09/07/session-parent/subagent/child-session/session.jsonl":
                include_str!("../fixtures/subagent-child-synthetic.jsonl"),
        });
        let _cleanup = EnvVarGuard::set("MUSE_DATA_DIR", fixture.path("muse"));
        let entries =
            load_entries_inner(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 2);
        let mut session_ids = entries
            .iter()
            .map(|entry| entry.session_id.to_string())
            .collect::<Vec<_>>();
        session_ids.sort();
        assert_eq!(session_ids, ["child-session", "session-parent"]);

        let rows =
            crate::summarize_entries(&entries, crate::cli::AgentReportKind::Session).unwrap();
        let report = crate::report_from_rows(&rows, crate::cli::AgentReportKind::Session);
        assert_eq!(report["sessions"].as_array().unwrap().len(), 2);
        assert_eq!(report["totals"]["inputTokens"], 16);
        assert_eq!(report["totals"]["outputTokens"], 4);
    }

    fn completion_record(stream_id: &str, record_id: &str, recorded_at: u64) -> String {
        format!(
            r#"{{"recorded_at":{recorded_at},"stream":{{"id":"{stream_id}"}},"id":"{record_id}","payload_type":"runtime.session","payload":{{"event":{{"kind":"model_completed","model":"muse-spark-1.2","usage":{{"input_tokens":10,"output_tokens":2,"cache_read_tokens":2}}}}}}}}"#
        )
    }

    fn completion_without_model(recorded_at: u64) -> String {
        format!(
            r#"{{"recorded_at":{recorded_at},"payload_type":"runtime.session","payload":{{"event":{{"kind":"model_completed","usage":{{"input_tokens":1}}}}}}}}"#
        )
    }
}
