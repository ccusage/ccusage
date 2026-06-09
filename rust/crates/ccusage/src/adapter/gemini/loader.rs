use crate::{LoadedEntry, PricingMap, Result, cli::SharedArgs, debug_log, parse_tz};

use super::{
    parser::{event_to_loaded, parse_json_file, parse_jsonl_file},
    paths::discover_log_files,
};

pub(crate) fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(crate::progress::UsageLoadAgent::Gemini, shared.json, || {
        load_entries_inner(shared, pricing)
    })
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let files = discover_log_files()?;
    let mut entries = crate::cache::load_with_cache(
        "gemini",
        &files,
        shared.single_thread,
        crate::cache::cost_fingerprint(Some(pricing.fingerprint()), shared.mode),
        shared.live_only,
        |file| {
            Ok({
                let events = if file.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    parse_jsonl_file(file)
                } else {
                    parse_json_file(file)
                };
                events
                    .map(|events| {
                        events
                            .into_iter()
                            .map(|event| event_to_loaded(event, tz.as_ref(), shared.mode, pricing))
                            .collect()
                    })
                    .unwrap_or_else(|error| {
                        debug_log(
                            shared,
                            format!("Failed to read Gemini log file {}: {error}", file.display()),
                        );
                        Vec::new()
                    })
            })
        },
    )?;
    entries.sort_by_key(|e| e.timestamp);
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::tests::CacheEnv;
    use ccusage_test_support::fs_fixture;

    #[test]
    fn loads_jsonl_token_events_and_separates_cached_input() {
        let _cache_env = CacheEnv::new("gemini-loads-jsonl");
        let _guard = super::super::GEMINI_DATA_DIR_LOCK.lock().unwrap();
        let fixture = fs_fixture!({
            "project/chats/session-a.jsonl": [
                r#"{"sessionId":"session-a","projectHash":"project-a","startTime":"2026-05-17T11:07:00.000Z"}"#,
                r#"{"id":"msg-a","timestamp":"2026-05-17T11:07:32.000Z","type":"gemini","model":"gemini-3-flash-preview","tokens":{"input":15327,"output":23,"cached":11526,"thoughts":919,"tool":7,"total":16276}}"#,
            ]
            .join("\n"),
        });
        let _env_guard = super::super::GeminiDataDirEnvGuard::set(fixture.root());
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries(&shared, &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2026-05-17");
        assert_eq!(entries[0].session_id.as_ref(), "session-a");
        assert_eq!(entries[0].model.as_deref(), Some("gemini-3-flash-preview"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 3_808);
        assert_eq!(entries[0].data.message.usage.output_tokens, 23);
        assert_eq!(
            entries[0].data.message.usage.cache_read_input_tokens,
            11_526
        );
        assert_eq!(entries[0].extra_total_tokens, 919);
    }
}
