use std::{collections::HashSet, path::PathBuf};

use crate::{
    LoadedEntry, PricingMap, Result, cli::SharedArgs, debug_log, parse_tz, read_files_parallel,
};

use super::{
    parser::{entry_id, parse_session_files},
    paths::{GrokSessionFiles, collect_session_files, paths},
};

pub(crate) fn load_entries(
    shared: &SharedArgs,
    custom_path: Option<&str>,
    pricing: Option<&PricingMap>,
) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(crate::progress::UsageLoadAgent::Grok, shared.json, || {
        load_entries_inner(shared, custom_path, pricing)
    })
}

fn load_entries_inner(
    shared: &SharedArgs,
    custom_path: Option<&str>,
    pricing: Option<&PricingMap>,
) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for root in paths(custom_path) {
        let session_files = collect_session_files(&root)?;
        let update_paths = session_files
            .iter()
            .map(|file| file.updates.clone())
            .collect::<Vec<PathBuf>>();
        // Parallel reads take PathBuf; reattach optional sibling summary.json per file.
        let loaded = read_files_parallel(&update_paths, shared.single_thread, |updates| {
            let summary = updates
                .parent()
                .map(|parent| parent.join("summary.json"))
                .filter(|candidate| candidate.is_file());
            let files = GrokSessionFiles {
                updates: updates.to_path_buf(),
                summary,
            };
            parse_session_files(&files, tz.as_ref(), shared.mode, pricing).unwrap_or_else(|error| {
                debug_log(
                    shared,
                    format!(
                        "Failed to read Grok session file {}: {error}",
                        updates.display()
                    ),
                );
                Vec::new()
            })
        });
        for file_entries in loaded {
            for entry in file_entries {
                if seen.insert(entry_id(&entry)) {
                    entries.push(entry);
                }
            }
        }
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    use super::super::paths::GROK_HOME_ENV;

    static GROK_HOME_LOCK: Mutex<()> = Mutex::new(());

    fn turn_line(event_id: &str, model: &str, input: u64, output: u64) -> String {
        format!(
            r#"{{"timestamp":1783903216,"params":{{"sessionId":"sess","update":{{"sessionUpdate":"turn_completed","usage":{{"modelUsage":{{"{model}":{{"inputTokens":{input},"outputTokens":{output},"reasoningTokens":0}}}}}}}},"_meta":{{"eventId":"{event_id}"}}}}}}"#
        )
    }

    #[test]
    fn loads_and_dedupes_by_event_id() {
        let _guard = GROK_HOME_LOCK.lock().unwrap();
        let line = turn_line("evt-dup", "grok-4.5", 10, 2);
        let fixture = fs_fixture!({
            "sessions/p/s1/updates.jsonl": format!("{line}\n{line}\n"),
        });
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            mode: crate::cli::CostMode::Calculate,
            offline: true,
            ..SharedArgs::default()
        };
        let pricing = PricingMap::load_with_overrides(true, false, std::iter::empty());
        let entries = load_entries(&shared, fixture.root().to_str(), Some(&pricing)).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("[grok] grok-4.5"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 10);
    }

    #[test]
    fn priced_model_cost_matches_hand_calculation() {
        let _guard = GROK_HOME_LOCK.lock().unwrap();
        // input 1000 @ 2e-6, output 500 @ 6e-6, reasoning 100 billed as output
        let line = r#"{"timestamp":1783903216,"params":{"sessionId":"s","update":{"sessionUpdate":"turn_completed","usage":{"modelUsage":{"grok-4.5":{"inputTokens":1000,"outputTokens":500,"reasoningTokens":100}}}},"_meta":{"eventId":"e"}}}"#;
        let fixture = fs_fixture!({
            "sessions/p/s/updates.jsonl": line,
        });
        let shared = SharedArgs {
            mode: crate::cli::CostMode::Calculate,
            offline: true,
            ..SharedArgs::default()
        };
        let pricing = PricingMap::load_with_overrides(true, false, std::iter::empty());
        let entries = load_entries(&shared, fixture.root().to_str(), Some(&pricing)).unwrap();
        assert_eq!(entries.len(), 1);
        // 1000*2e-6 + 600*6e-6 = 0.002 + 0.0036 = 0.0056
        assert!(
            (entries[0].cost - 0.0056).abs() < 1e-9,
            "cost={}",
            entries[0].cost
        );
        assert_eq!(entries[0].data.message.usage.output_tokens, 500);
        assert_eq!(entries[0].extra_total_tokens, 100);
    }

    #[test]
    fn grok_home_env_is_used_when_no_custom_path() {
        let _guard = GROK_HOME_LOCK.lock().unwrap();
        let fixture = fs_fixture!({
            "sessions/p/s/updates.jsonl": turn_line("e", "grok-4.5", 1, 1),
        });
        let _cleanup = EnvVarGuard::set(GROK_HOME_ENV, fixture.root());
        let shared = SharedArgs {
            offline: true,
            mode: crate::cli::CostMode::Calculate,
            ..SharedArgs::default()
        };
        let pricing = PricingMap::load_with_overrides(true, false, std::iter::empty());
        let entries = load_entries(&shared, None, Some(&pricing)).unwrap();
        assert_eq!(entries.len(), 1);
    }
}
