use std::collections::HashSet;

use crate::{
    LoadedEntry, PricingMap, Result, cli::SharedArgs, debug_log, parse_tz, read_files_parallel,
};

use super::{
    parser::{dsh_entry_key, dsh_entry_to_loaded, read_session_file},
    paths::discover_session_files,
};

pub fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent("DeepSeek Harness"),
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
                    "Failed to read DeepSeek Harness session {}: {error}",
                    file.display()
                ),
            );
            Vec::new()
        })
    });
    // The same session can be reachable from more than one DSH_HOME root.
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for file_entries in loaded {
        for entry in file_entries {
            if seen.insert(dsh_entry_key(&entry)) {
                entries.push(dsh_entry_to_loaded(
                    entry,
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

#[cfg(test)]
mod tests {
    use std::fs;

    use super::super::paths::DSH_HOME_ENV;
    use super::*;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    const SESSION: &str = concat!(
        r#"{"type":"session","version":4,"id":"sid-1","createdAt":1785015039278,"cwd":"/home/u/proj"}"#,
        "\n",
        r#"{"type":"request/header","seq":3,"time":1785015040000,"data":{"header":{"config":{"provider":"deepseek-official","model":"deepseek-v4-pro"}},"reason":"initial"}}"#,
        "\n",
        r#"{"type":"assistant/message","seq":5,"time":1785015045000,"data":{"usage":{"inputTokens":1200,"outputTokens":300,"cacheReadTokens":8000,"totalTokens":9500}}}"#,
        "\n",
    );

    #[test]
    fn loads_zstd_sessions_split_across_frames_and_tolerates_a_torn_tail() {
        let fixture = fs_fixture!({ "sessions/--home-u-proj--/sid-1/.keep": "" });
        let (header, events) = SESSION.split_once('\n').unwrap();
        let mut compressed = zstd::stream::encode_all(format!("{header}\n").as_bytes(), 3).unwrap();
        compressed.extend(zstd::stream::encode_all(events.as_bytes(), 3).unwrap());
        let torn = zstd::stream::encode_all(
            br#"{"type":"assistant/message","seq":9,"data":{"usage":{"inputTokens":1}}}"#
                .as_slice(),
            3,
        )
        .unwrap();
        compressed.extend(&torn[..torn.len() / 2]);
        fs::write(
            fixture.path("sessions/--home-u-proj--/sid-1/session.v4.jsonl.zstd"),
            compressed,
        )
        .unwrap();
        let _cleanup = EnvVarGuard::set(DSH_HOME_ENV, fixture.root());
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        let entries = load_entries(&shared, &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2026-07-25");
        assert_eq!(entries[0].session_id.as_ref(), "sid-1");
        assert_eq!(entries[0].model.as_deref(), Some("deepseek-v4-pro"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 1200);
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 8000);
        assert!(entries[0].cost > 0.0);
    }

    #[test]
    fn reads_only_the_newest_generation_and_dedupes_across_roots() {
        let first = fs_fixture!({
            "sessions/p/sid-1/session.v3.jsonl": SESSION,
            "sessions/p/sid-1/session.v4.jsonl": SESSION,
        });
        let second = fs_fixture!({ "sessions/p/sid-1/session.v4.jsonl": SESSION });
        let value = format!("{},{}", first.root().display(), second.root().display());
        let _cleanup = EnvVarGuard::set(DSH_HOME_ENV, &value);

        let entries = load_entries(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
    }
}
