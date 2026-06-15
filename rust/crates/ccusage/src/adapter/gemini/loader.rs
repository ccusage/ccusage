use crate::{
    LoadedEntry, PricingMap, Result, cli::SharedArgs, debug_log, parse_tz, read_files_parallel,
};

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
    // Read each log file in parallel; the events keep their original file order
    // before the stable sort, so output is identical to the sequential read.
    let loaded = read_files_parallel(&files, shared.single_thread, |file| {
        let parsed = if file.extension().and_then(|extension| extension.to_str()) == Some("jsonl") {
            parse_jsonl_file(file)
        } else {
            parse_json_file(file)
        };
        parsed.unwrap_or_else(|error| {
            debug_log(
                shared,
                format!("Failed to read Gemini log file {}: {error}", file.display()),
            );
            Vec::new()
        })
    });
    let mut events: Vec<_> = loaded.into_iter().flatten().collect();
    events.sort_by_key(|event| event.timestamp);
    Ok(events
        .into_iter()
        .map(|event| event_to_loaded(event, tz.as_ref(), shared.mode, pricing))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::fs_fixture;

    #[test]
    fn loads_jsonl_token_events_and_separates_cached_input() {
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
