use std::collections::HashSet;

use crate::{
    LoadedEntry, PricingMap, Result, cli::SharedArgs, debug_log, parse_tz, read_files_parallel,
};

use super::{
    parser::{entry_id, migration_id, parse_session_file},
    paths::{collect_session_files, paths},
    sqlite::collect_sqlite_entries,
};

pub fn load_entries(
    shared: &SharedArgs,
    custom_path: Option<&str>,
    pricing: Option<&PricingMap>,
) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent("OpenClaw"),
        shared.json,
        || load_entries_inner(shared, custom_path, pricing),
    )
}

fn load_entries_inner(
    shared: &SharedArgs,
    custom_path: Option<&str>,
    pricing: Option<&PricingMap>,
) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    // SQLite-era event ids (`session_id`, message `id`) win over the
    // content-based JSONL ids: after `openclaw doctor --fix` migrates legacy
    // JSONL into per-agent SQLite, both copies of the same event exist on
    // disk, and the SQLite row carries the provider-billed embedded cost.
    let mut sqlite_ids = HashSet::new();
    let mut sqlite_entries: Vec<LoadedEntry> = Vec::new();
    for root in paths(custom_path) {
        sqlite_entries.extend(collect_sqlite_entries(
            &root,
            tz.as_ref(),
            shared.mode,
            pricing,
            &|message| debug_log(shared, message),
        ));
        let files = collect_session_files(&root)?;
        // Read session files in parallel; the first-wins dedup runs sequentially
        // over the original file order so the surviving record per id is the
        // same as the single-threaded read.
        let loaded = read_files_parallel(&files, shared.single_thread, |file| {
            parse_session_file(file, tz.as_ref(), shared.mode, pricing).unwrap_or_else(|error| {
                debug_log(
                    shared,
                    format!(
                        "Failed to read OpenClaw session file {}: {error}",
                        file.display()
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
    for entry in sqlite_entries {
        if let Some(message_id) = entry.data.message.id.clone() {
            let sqlite_id = format!("openclaw:{}:{message_id}", entry.session_id);
            if sqlite_ids.insert(sqlite_id) {
                // A migrated JSONL duplicate carries no stable id, but it
                // still shares the session, timestamp, model, and usage. Its
                // cost can differ, so compare the migration identity without
                // cost and keep the provider-billed SQLite row.
                let content_id = entry_id(&entry);
                let migration_key = migration_id(&entry);
                if let Some(position) = entries.iter().position(|existing| {
                    existing.session_id == entry.session_id
                        && existing.data.message.id.as_deref() == Some(message_id.as_str())
                }) {
                    entries[position] = entry;
                } else if let Some(position) = entries
                    .iter()
                    .position(|existing| migration_id(existing) == migration_key)
                {
                    entries[position] = entry;
                } else {
                    seen.insert(content_id);
                    entries.push(entry);
                }
            }
        } else if seen.insert(entry_id(&entry)) {
            entries.push(entry);
        }
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use ccusage_test_support::fs_fixture;

    static OPENCLAW_DIR_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn loads_assistant_usage_and_uses_model_change_events() {
        let _guard = OPENCLAW_DIR_LOCK.lock().unwrap();
        let fixture = fs_fixture!({
            "agents/main/sessions/abc.jsonl": [
                r#"{"type":"model_change","provider":"openai-codex","modelId":"gpt-5.2"}"#,
                r#"{"type":"message","message":{"role":"assistant","usage":{"input":1660,"output":55,"cacheRead":108928,"cost":{"total":0.02}},"timestamp":1769753935279}}"#,
            ]
            .join("\n"),
        });
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries(&shared, fixture.root().to_str(), None).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2026-01-30");
        assert_eq!(entries[0].session_id.as_ref(), "abc");
        assert_eq!(entries[0].model.as_deref(), Some("[openclaw] gpt-5.2"));
        assert_eq!(entries[0].data.version.as_deref(), Some("openai-codex"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 1660);
        assert_eq!(entries[0].data.message.usage.output_tokens, 55);
        assert_eq!(
            entries[0].data.message.usage.cache_read_input_tokens,
            108_928
        );
        assert_eq!(entries[0].extra_total_tokens, 0);
        assert!((entries[0].cost - 0.02).abs() < f64::EPSILON);
    }

    #[test]
    fn deduplicates_repeated_openclaw_records() {
        let _guard = OPENCLAW_DIR_LOCK.lock().unwrap();
        let line = r#"{"type":"message","message":{"role":"assistant","model":"gpt-5.2","usage":{"input":1,"output":1,"totalTokens":2},"timestamp":1769753935279}}"#;
        let fixture = fs_fixture!({
            "agents/main/sessions/session.jsonl": format!("{line}\n{line}\n"),
        });
        let entries = load_entries(&SharedArgs::default(), fixture.root().to_str(), None).unwrap();

        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn calculates_cost_from_pricing_overrides() {
        let _guard = OPENCLAW_DIR_LOCK.lock().unwrap();
        let fixture = fs_fixture!({
            "agents/main/sessions/abc.jsonl": r#"{"type":"message","message":{"role":"assistant","model":"gpt-5.2","usage":{"input":1000,"output":500,"cost":{"total":0.99}},"timestamp":1769753935279}}"#,
        });
        let mut shared = SharedArgs {
            mode: crate::cli::CostMode::Calculate,
            offline: true,
            ..SharedArgs::default()
        };
        shared.pricing_overrides.insert(
            "[openclaw] gpt-5.2".to_string(),
            ccusage_cli::PricingOverride {
                input_cost_per_token: Some(1e-6),
                output_cost_per_token: Some(2e-6),
                ..Default::default()
            },
        );
        let pricing =
            PricingMap::load_with_overrides(shared.offline, false, shared.pricing_overrides.iter());

        let entries = load_entries(&shared, fixture.root().to_str(), Some(&pricing)).unwrap();

        assert_eq!(entries.len(), 1);
        assert!((entries[0].cost - 0.002).abs() < f64::EPSILON);
        assert_eq!(entries[0].data.cost_usd, Some(0.99));
    }

    #[test]
    fn loads_sqlite_usage_when_no_jsonl_sessions_exist() {
        let _guard = OPENCLAW_DIR_LOCK.lock().unwrap();
        let fixture = fs_fixture!({});
        let db_path = fixture.path("agents/main/agent/openclaw-agent.sqlite");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let db = sqlite::open(&db_path).unwrap();
        db.execute(
            "CREATE TABLE transcript_events (session_id TEXT NOT NULL, seq INTEGER NOT NULL, event_json TEXT NOT NULL, created_at INTEGER NOT NULL, PRIMARY KEY (session_id, seq))",
        )
        .unwrap();
        for (seq, event_json) in [
            r#"{"id":"evt-1","type":"model_change","modelId":"deepseek-v4-flash","provider":"deepseek"}"#,
            r#"{"id":"evt-2","type":"message","message":{"role":"assistant","usage":{"input":100,"output":50,"cost":{"total":1.80}},"timestamp":1769753935279}}"#,
        ]
        .into_iter()
        .enumerate()
        {
            let mut statement = db
                .prepare(
                    "INSERT INTO transcript_events (session_id, seq, event_json, created_at) VALUES ('session-sqlite', ?1, ?2, 1769753935279)",
                )
                .unwrap();
            statement.bind((1, seq as i64)).unwrap();
            statement.bind((2, event_json)).unwrap();
            statement.next().unwrap();
        }
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        let entries = load_entries(&shared, fixture.root().to_str(), None).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2026-01-30");
        assert_eq!(entries[0].session_id.as_ref(), "session-sqlite");
        assert_eq!(
            entries[0].model.as_deref(),
            Some("[openclaw] deepseek-v4-flash")
        );
        assert_eq!(entries[0].data.version.as_deref(), Some("deepseek"));
        assert!((entries[0].cost - 1.80).abs() < f64::EPSILON);
        assert_eq!(entries[0].data.message.id.as_deref(), Some("evt-2"));
    }

    #[test]
    fn prefers_sqlite_over_migrated_jsonl_duplicates() {
        let _guard = OPENCLAW_DIR_LOCK.lock().unwrap();
        let line = r#"{"type":"message","message":{"role":"assistant","model":"gpt-5.2","usage":{"input":10,"output":20,"cost":{"total":0.50}},"timestamp":1769753935279}}"#;
        let fixture = fs_fixture!({
            "agents/main/sessions/session.jsonl": format!("{line}\n"),
        });
        let db_path = fixture.path("agents/main/agent/openclaw-agent.sqlite");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let db = sqlite::open(&db_path).unwrap();
        db.execute(
            "CREATE TABLE transcript_events (session_id TEXT NOT NULL, seq INTEGER NOT NULL, event_json TEXT NOT NULL, created_at INTEGER NOT NULL, PRIMARY KEY (session_id, seq))",
        )
        .unwrap();
        let mut statement = db
            .prepare(
                "INSERT INTO transcript_events (session_id, seq, event_json, created_at) VALUES ('session', 0, ?1, 1769753935279)",
            )
            .unwrap();
        statement
            .bind((
                1,
                r#"{"id":"evt-sqlite","type":"message","message":{"role":"assistant","model":"gpt-5.2","usage":{"input":10,"output":20,"cost":{"total":0.51}},"timestamp":1769753935279}}"#,
            ))
            .unwrap();
        statement.next().unwrap();

        let entries = load_entries(&SharedArgs::default(), fixture.root().to_str(), None).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.id.as_deref(), Some("evt-sqlite"));
        assert!((entries[0].cost - 0.51).abs() < f64::EPSILON);
    }
}
