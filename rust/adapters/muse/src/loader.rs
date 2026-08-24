use std::{collections::HashSet, path::Path};

use crate::{LoadedEntry, PricingMap, Result, cli::SharedArgs, read_files_parallel};

use super::{
    parser::{MuseEntry, MuseSession, parse_session_file, to_loaded_entry},
    paths::muse_session_files,
};

pub fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent("Muse Code"),
        shared.json,
        || load_entries_inner(shared, pricing),
    )
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let tz = crate::parse_tz(shared.timezone.as_deref());
    let files = muse_session_files()?;
    let loaded = read_files_parallel(&files, shared.single_thread, |file| {
        load_session_file(file, shared)
    });
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for (file_entries, workspace_root) in loaded {
        for entry in file_entries {
            // Record ids repeat across sessions, so the (session, id) pair is
            // the stable identity a re-read of the same log must dedupe on.
            let key = (entry.session_id.clone(), entry.record_id.clone());
            if !seen.insert(key) {
                continue;
            }
            entries.push(to_loaded_entry(
                entry,
                tz.as_ref(),
                pricing,
                workspace_root.as_deref(),
            ));
        }
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

fn load_session_file(file_path: &Path, shared: &SharedArgs) -> (Vec<MuseEntry>, Option<String>) {
    let Ok(contents) = std::fs::read_to_string(file_path) else {
        crate::debug_log(
            shared,
            format!(
                "Failed to read Muse Code session log: {}",
                file_path.display()
            ),
        );
        return (Vec::new(), None);
    };
    let session: MuseSession = parse_session_file(&contents);
    (session.entries, session.workspace_root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    fn envelope(
        payload_type: &str,
        id: &str,
        stream_id: &str,
        recorded_at_us: u64,
        event: &str,
    ) -> String {
        format!(
            r#"{{"schema_version":1,"id":"{id}","stream":{{"kind":"session","id":"{stream_id}"}},"sequence":1,"recorded_at":{recorded_at_us},"record_type":"event","durability":"durable","causation_id":null,"payload_type":"{payload_type}","payload_schema_version":1,"payload":{event}}}"#
        )
    }

    fn model_completed(id: &str, stream_id: &str, recorded_at_us: u64, model: &str) -> String {
        envelope(
            "runtime.session",
            id,
            stream_id,
            recorded_at_us,
            &format!(
                r#"{{"event":{{"kind":"model_completed","model":"{model}","usage":{{"input_tokens":100,"output_tokens":10}}}},"kind":"run"}}"#
            ),
        )
    }

    #[test]
    fn loads_entries_from_session_and_subagent_logs() {
        let fixture = fs_fixture!({
            "muse/sessions/2026/08/01/sess-parent/session.jsonl": [
                envelope(
                    "runtime.session.metadata",
                    "meta-1",
                    "sess-parent",
                    1_785_962_827_173_826,
                    r#"{"record":{"workspace_root":"/home/user/projects/ccusage"}}"#,
                ),
                model_completed("rec-1", "sess-parent", 1_785_962_827_173_826, "muse-spark-1.2"),
                model_completed("rec-2", "sess-parent", 1_785_962_828_000_000, "muse-spark-1.2"),
            ]
            .join("\n"),
            "muse/sessions/2026/08/01/sess-parent/subagent/sess-child/session.jsonl":
                model_completed("rec-3", "sess-child", 1_785_962_830_000_000, "muse-spark-1.2-contributor"),
        });
        let _xdg = EnvVarGuard::set("XDG_DATA_HOME", fixture.root());
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            mode: crate::cli::CostMode::Display,
            ..SharedArgs::default()
        };

        let entries = load_entries(&shared, &PricingMap::default()).unwrap();

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].session_id.as_ref(), "sess-parent");
        assert_eq!(entries[0].project.as_ref(), "ccusage");
        assert_eq!(entries[2].session_id.as_ref(), "sess-child");
        // The child log has no metadata record of its own, so it falls back to
        // the source name.
        assert_eq!(entries[2].project.as_ref(), "muse");
    }

    #[test]
    fn dedupes_records_on_reread() {
        let fixture = fs_fixture!({
            "muse/sessions/2026/08/01/sess-parent/session.jsonl": [
                model_completed("rec-1", "sess-parent", 1_785_962_827_173_826, "muse-spark-1.2"),
                model_completed("rec-1", "sess-parent", 1_785_962_827_173_826, "muse-spark-1.2"),
            ]
            .join("\n"),
        });
        let _xdg = EnvVarGuard::set("XDG_DATA_HOME", fixture.root());
        let shared = SharedArgs::default();

        let entries = load_entries(&shared, &PricingMap::default()).unwrap();

        assert_eq!(entries.len(), 1);
    }
}
