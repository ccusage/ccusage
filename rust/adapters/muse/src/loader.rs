use std::{
    collections::{HashMap, HashSet},
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
    let loaded = read_session_files(&files, shared.single_thread, shared)?;

    let mut unique_entries = Vec::<Option<(String, MuseUsageEntry)>>::new();
    let mut seen = HashMap::<RecordKey, usize>::new();
    let mut rejected_keys = HashSet::new();
    for (file, records) in files.iter().zip(loaded) {
        rejected_keys.extend(records.rejected_keys);
        let session_id = session_path_info(file);
        for entry in records.entries {
            let Some(key) = record_key(&entry) else {
                continue;
            };
            if let Some(index) = seen.get(&key).copied() {
                if let Some((_, previous)) = &unique_entries[index]
                    && !previous.has_same_required_payload(&entry)
                {
                    unique_entries[index] = None;
                }
            } else {
                seen.insert(key, unique_entries.len());
                unique_entries.push(Some((session_id.clone(), entry)));
            }
        }
    }
    let mut entries = unique_entries
        .into_iter()
        .flatten()
        .filter(|(_, entry)| {
            entry.is_meta_routed()
                && record_key(entry).is_some_and(|key| !rejected_keys.contains(&key))
        })
        .map(|(session_id, entry)| {
            muse_entry_to_loaded(entry, &session_id, tz.as_ref(), shared.mode, pricing)
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

fn read_session_files(
    files: &[std::path::PathBuf],
    single_thread: bool,
    shared: &SharedArgs,
) -> Result<Vec<SessionFileRecords>> {
    read_files_parallel(files, single_thread, |file| {
        let result = read_session_file(file);
        if let Err(error) = &result {
            debug_log(
                shared,
                format!(
                    "Failed to read Muse session file {}: {error}",
                    file.display()
                ),
            );
        }
        result
    })
    .into_iter()
    .collect()
}

fn read_session_file(path: &Path) -> Result<SessionFileRecords> {
    let content = fs::read(path)?;
    let prefilter =
        LinePrefilter::any(&[br#""model_completed""#, br#""runtime.session.metadata""#]);
    let mut records = SessionFileRecords::default();
    let mut current_metadata = None;
    for value in jsonl::records::<Value>(&content, Some(&prefilter)) {
        if let Some(metadata) = parse_metadata_record(&value) {
            current_metadata = Some(metadata);
            continue;
        }
        if let Some(mut entry) = parse_model_completed_value(&value) {
            if let Some(metadata) = &current_metadata {
                metadata.apply_to(&mut entry);
            }
            records.entries.push(entry);
        } else if let Some(key) = record_key_from_value(&value) {
            records.rejected_keys.push(key);
        }
    }
    Ok(records)
}

#[derive(Default)]
struct SessionFileRecords {
    entries: Vec<MuseUsageEntry>,
    rejected_keys: Vec<RecordKey>,
}

#[derive(Debug, Hash, PartialEq, Eq)]
struct RecordKey {
    stream_id: String,
    record_id: String,
}

fn record_key(entry: &MuseUsageEntry) -> Option<RecordKey> {
    match (&entry.stream_id, &entry.record_id) {
        (Some(stream_id), Some(record_id))
            if !stream_id.trim().is_empty() && !record_id.trim().is_empty() =>
        {
            Some(RecordKey {
                stream_id: stream_id.clone(),
                record_id: record_id.clone(),
            })
        }
        _ => None,
    }
}

fn record_key_from_value(value: &Value) -> Option<RecordKey> {
    if value.get("payload_type")?.as_str()? != "runtime.session"
        || value.get("payload")?.get("event")?.get("kind")?.as_str()? != "model_completed"
    {
        return None;
    }
    let identity = |value: &Value| {
        let id = value.as_str()?;
        (!id.trim().is_empty()).then(|| id.to_owned())
    };
    Some(RecordKey {
        stream_id: identity(value.get("stream")?.get("id")?)?,
        record_id: identity(value.get("id")?)?,
    })
}

fn session_path_info(file: &Path) -> String {
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
    relative
        .last()
        .or_else(|| components.last())
        .cloned()
        .unwrap_or_else(|| "unknown".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_model_completed_value;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    #[test]
    fn deduplicates_copied_roots_by_stream_and_record_identity() {
        let fixture = fs_fixture!({
            "muse/sessions/2026/09/07/session-a/session.jsonl": [
                completion_record("stream-a", "record-a", 1_000_000),
                completion_record("stream-a", "record-b", 1_000_000),
                r#"{"recorded_at":1000000,"stream":{"id":"stream-a"},"id":"partial","payload_type":"runtime.session","payload":{"event":{"kind":"model_streaming","usage":{"input_tokens":999,"output_tokens":999}}}}"#.to_owned(),
                r#"{"recorded_at":1000000,"stream":{"id":"stream-a"},"id":"total","payload_type":"runtime.session","payload":{"event":{"kind":"session_usage","usage":{"input_tokens":999,"output_tokens":999}}}}"#.to_owned(),
            ]
            .join("\n"),
            "backup/muse/sessions/2026/10/01/copied-session/session.jsonl": [
                completion_record("stream-a", "record-a", 1_000_000),
                completion_record("stream-b", "record-a", 1_000_000),
            ]
            .join("\n"),
        });
        let _cleanup = EnvVarGuard::set(
            "MUSE_DATA_DIR",
            format!(
                "{},{}",
                fixture.path("muse").display(),
                fixture.path("backup/muse").display()
            ),
        );
        let entries =
            load_entries_inner(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 3);
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
    fn excludes_records_without_both_opaque_identity_fields() {
        let mut record: Value =
            serde_json::from_str(&completion_record("stream-a", "record-a", 1_000_000)).unwrap();
        record.as_object_mut().unwrap().remove("id");
        record.as_object_mut().unwrap().remove("stream");
        let record = record.to_string();
        let fixture = fs_fixture!({
            "muse/sessions/2026/09/07/session-a/session.jsonl": format!("{record}\n{record}\n"),
        });
        let _cleanup = EnvVarGuard::set("MUSE_DATA_DIR", fixture.path("muse"));
        let entries =
            load_entries_inner(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        assert!(entries.is_empty());
    }

    #[test]
    fn excludes_all_copies_when_a_stable_identity_has_conflicting_payload_or_route() {
        let fixture = fs_fixture!({
            "muse/sessions/2026/09/07/session-a/session.jsonl": [
                completion_record("stream-a", "timestamp-conflict", 1_000_000),
                completion_record("stream-a", "timestamp-conflict", 1_000_001),
                completion_record("stream-a", "timestamp-conflict", 1_000_000),
                completion_record("stream-a", "token-conflict", 1_000_000),
                completion_with_output("stream-a", "token-conflict", 1_000_000, 3),
                completion_record("stream-a", "model-conflict", 1_000_000),
                completion_with_model(
                    "stream-a",
                    "model-conflict",
                    1_000_000,
                    "muse-spark-1.2-contributor",
                    "meta",
                ),
                completion_record("stream-a", "route-conflict", 1_000_000),
                completion_with_provider("stream-a", "route-conflict", 1_000_000, "openrouter"),
                completion_record("stream-a", "independent", 1_000_000),
            ]
            .join("\n"),
        });
        let _cleanup = EnvVarGuard::set("MUSE_DATA_DIR", fixture.path("muse"));
        let entries =
            load_entries_inner(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.id.as_deref(), Some("independent"));
    }

    #[test]
    fn invalid_completion_copies_poison_their_stable_identity() {
        let fixture = fs_fixture!({
            "muse/sessions/2026/09/07/session-a/session.jsonl": [
                completion_record("stream-a", "missing-token", 1_000_000),
                without_record_field(
                    completion_record("stream-a", "missing-token", 1_000_000),
                    &["payload", "event", "usage", "output_tokens"],
                ),
                completion_record("stream-a", "nonzero-write", 2_000_000),
                with_record_value(
                    completion_record("stream-a", "nonzero-write", 2_000_000),
                    &["payload", "event", "usage", "cache_write_tokens"],
                    serde_json::json!(1),
                ),
                completion_record("stream-a", "bad-timestamp", 3_000_000),
                with_record_value(
                    completion_record("stream-a", "bad-timestamp", 3_000_000),
                    &["recorded_at"],
                    serde_json::json!("bad"),
                ),
                completion_record("stream-a", "independent", 4_000_000),
            ]
            .join("\n"),
        });
        let _cleanup = EnvVarGuard::set("MUSE_DATA_DIR", fixture.path("muse"));
        let entries =
            load_entries_inner(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.id.as_deref(), Some("independent"));
    }

    #[test]
    fn event_provider_overrides_metadata_in_both_directions() {
        let fixture = fs_fixture!({
            "muse/sessions/2026/09/07/session-a/session.jsonl": [
                metadata_record("muse-spark-1.2", "meta"),
                completion_with_provider("stream-a", "explicit-openrouter", 1_000_000, "openrouter"),
                metadata_record("muse-spark-1.2", "openrouter"),
                completion_with_provider("stream-a", "explicit-meta", 2_000_000, "meta"),
                metadata_record("muse-spark-1.2", "meta"),
                completion_without_provider("stream-a", "paired-meta", 3_000_000),
                completion_with_provider("stream-a", "explicit-unknown", 3_500_000, "unknown"),
                metadata_record("muse-spark-1.3", "meta"),
                completion_without_provider("stream-a", "mismatched-metadata", 4_000_000),
            ]
            .join("\n"),
        });
        let _cleanup = EnvVarGuard::set("MUSE_DATA_DIR", fixture.path("muse"));
        let entries =
            load_entries_inner(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        let ids = entries
            .iter()
            .filter_map(|entry| entry.data.message.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["explicit-meta", "paired-meta"]);
    }

    #[test]
    fn ignores_unrelated_nested_values_in_metadata_envelopes() {
        let fixture = fs_fixture!({
            "muse/sessions/2026/09/07/session-a/session.jsonl": [
                r#"{"payload_type":"runtime.session.metadata","payload":{"context":{"model":"unrelated-model","provider":"unrelated-provider","cwd":"/unrelated/project"}}}"#.to_owned(),
                completion_record("stream-a", "record-a", 1_000_000),
            ]
            .join("\n"),
        });
        let _cleanup = EnvVarGuard::set("MUSE_DATA_DIR", fixture.path("muse"));
        let entries =
            load_entries_inner(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("muse-spark-1.2"));
        assert_eq!(entries[0].project_path.as_ref(), "Muse Code");
    }

    #[test]
    fn applies_only_preceding_exact_metadata_snapshots() {
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
                    "muse-spark-1.2": { "input_cost_per_token": 0.2, "output_cost_per_token": 0 },
                    "meta/muse-spark-1.2-contributor": { "input_cost_per_token": 0.5, "output_cost_per_token": 0 }
                }"#
            ),
            2
        );
        let shared = SharedArgs {
            mode: ccusage_core::cli::CostMode::Calculate,
            ..SharedArgs::default()
        };
        let entries = load_entries_inner(&shared, &pricing).unwrap();

        assert_eq!(entries.len(), 4);
        assert_eq!(
            entries[0].data.message.id.as_deref(),
            Some("event-model-primary")
        );
        assert_eq!(entries[0].model.as_deref(), Some("muse-spark-1.2"));
        assert_eq!(entries[0].project_path.as_ref(), "/workspace/initial");
        assert_eq!(entries[1].model.as_deref(), Some("muse-spark-1.2"));
        assert_eq!(entries[1].project_path.as_ref(), "/workspace/switched");
        assert_eq!(
            entries[2].model.as_deref(),
            Some("muse-spark-1.2-contributor")
        );
        assert_eq!(entries[2].project_path.as_ref(), "/workspace/switched");
        assert_eq!(entries[3].model.as_deref(), Some("muse-spark-1.2"));
        assert_eq!(entries[3].project_path.as_ref(), "/workspace/reset");
        assert!((entries[0].cost - 0.8).abs() < 0.000_001);
        assert!((entries[1].cost - 0.8).abs() < 0.000_001);
        assert!((entries[2].cost - 2.0).abs() < 0.000_001);
        assert!((entries[3].cost - 0.8).abs() < 0.000_001);
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

    #[test]
    fn rejects_the_observed_excerpt_without_a_record_identity() {
        let observed = serde_json::from_str::<Value>(include_str!(
            "../fixtures/issue-1694-model-completed.jsonl"
        ))
        .unwrap();
        let parsed = parse_model_completed_value(&observed).unwrap();
        assert!(parsed.stream_id.is_some());
        assert_eq!(parsed.record_id, None);
        assert!(record_key(&parsed).is_none());

        let fixture = fs_fixture!({
            "muse/sessions/2026/09/07/issue-1694/session.jsonl": include_str!(
                "../fixtures/issue-1694-model-completed.jsonl"
            ),
        });
        let _cleanup = EnvVarGuard::set("MUSE_DATA_DIR", fixture.path("muse"));
        let entries =
            load_entries_inner(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn strict_synthetic_fixture_counts_only_its_complete_control() {
        let fixture = fs_fixture!({
            "muse/sessions/2026/09/07/strict-synthetic/session.jsonl": include_str!(
                "../fixtures/strict-eligibility-synthetic.jsonl"
            ),
        });
        let _cleanup = EnvVarGuard::set("MUSE_DATA_DIR", fixture.path("muse"));
        let entries =
            load_entries_inner(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].data.message.id.as_deref(),
            Some("strict-synthetic-control")
        );
        assert_eq!(entries[0].data.message.usage.input_tokens, 610);
        assert_eq!(
            entries[0].data.message.usage.cache_read_input_tokens,
            27_121
        );
        assert_eq!(entries[0].data.message.usage.output_tokens, 20);
    }

    #[test]
    fn propagates_session_file_read_errors_without_returning_partial_files() {
        let fixture = fs_fixture!({
            "valid/session.jsonl": completion_record("stream-a", "record-a", 1_000_000),
        });
        let files = [
            fixture.path("valid/session.jsonl"),
            fixture.path("missing/session.jsonl"),
        ];

        assert!(read_session_files(&files, true, &SharedArgs::default()).is_err());
    }

    fn completion_record(stream_id: &str, record_id: &str, recorded_at: u64) -> String {
        completion_with_provider(stream_id, record_id, recorded_at, "meta")
    }

    fn completion_with_provider(
        stream_id: &str,
        record_id: &str,
        recorded_at: u64,
        provider: &str,
    ) -> String {
        completion_with_model(
            stream_id,
            record_id,
            recorded_at,
            "muse-spark-1.2",
            provider,
        )
    }

    fn completion_with_model(
        stream_id: &str,
        record_id: &str,
        recorded_at: u64,
        model: &str,
        provider: &str,
    ) -> String {
        serde_json::json!({
            "recorded_at": recorded_at,
            "stream": { "id": stream_id },
            "id": record_id,
            "payload_type": "runtime.session",
            "payload": { "event": {
                "kind": "model_completed",
                "model": model,
                "provider": provider,
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 2,
                    "cache_read_tokens": 2,
                    "cache_write_tokens": 0
                }
            } }
        })
        .to_string()
    }

    fn completion_with_output(
        stream_id: &str,
        record_id: &str,
        recorded_at: u64,
        output_tokens: u64,
    ) -> String {
        let mut record: Value =
            serde_json::from_str(&completion_record(stream_id, record_id, recorded_at)).unwrap();
        record["payload"]["event"]["usage"]["output_tokens"] = serde_json::json!(output_tokens);
        record.to_string()
    }

    fn with_record_value(record: String, path: &[&str], replacement: Value) -> String {
        let mut record: Value = serde_json::from_str(&record).unwrap();
        let mut parent = &mut record;
        for key in &path[..path.len() - 1] {
            parent = parent.get_mut(*key).unwrap();
        }
        parent
            .as_object_mut()
            .unwrap()
            .insert(path[path.len() - 1].to_owned(), replacement);
        record.to_string()
    }

    fn without_record_field(record: String, path: &[&str]) -> String {
        let mut record: Value = serde_json::from_str(&record).unwrap();
        let mut parent = &mut record;
        for key in &path[..path.len() - 1] {
            parent = parent.get_mut(*key).unwrap();
        }
        parent.as_object_mut().unwrap().remove(path[path.len() - 1]);
        record.to_string()
    }

    fn completion_without_provider(stream_id: &str, record_id: &str, recorded_at: u64) -> String {
        let mut value: Value =
            serde_json::from_str(&completion_record(stream_id, record_id, recorded_at)).unwrap();
        value["payload"]["event"]
            .as_object_mut()
            .unwrap()
            .remove("provider");
        value.to_string()
    }

    fn metadata_record(model_id: &str, provider_id: &str) -> String {
        serde_json::json!({
            "payload_type": "runtime.session.metadata",
            "payload": { "record": {
                "model_id": model_id,
                "provider_id": provider_id
            } }
        })
        .to_string()
    }
}
