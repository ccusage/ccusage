use crate::{
    LoadedEntry, PricingMap, Result, cli::SharedArgs, debug_log, parse_tz, read_files_parallel,
};

use super::{
    parser::{event_to_loaded, parse_json_file, parse_jsonl_file, parse_sqlite_file},
    paths::discover_log_files,
};

pub fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent("Gemini CLI"),
        shared.json,
        || load_entries_inner(shared, pricing),
    )
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let files = discover_log_files()?;
    // Read each log file in parallel; the events keep their original file order
    // before the stable sort, so output is identical to the sequential read.
    let loaded = read_files_parallel(&files, shared.single_thread, |file| {
        let parsed = match file.extension().and_then(|extension| extension.to_str()) {
            Some("jsonl") => parse_jsonl_file(file),
            Some("db") => parse_sqlite_file(file),
            _ => parse_json_file(file),
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

    #[test]
    fn loads_sqlite_antigravity_metadata() {
        let fixture = ccusage_test_support::Fixture::new();
        let db_path = fixture.path("session-db.db");
        let connection = sqlite::open(&db_path).unwrap();
        connection
            .execute("CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB);")
            .unwrap();

        // 1.4: tokens submessage (1.4.2 = 100, 1.4.3 = 50, 1.4.5 = 20, 1.4.9 = 10)
        let tokens_bytes = vec![
            0x10, 100, // 2: varint 100
            0x18, 50, // 3: varint 50
            0x28, 20, // 5: varint 20
            0x48, 10, // 9: varint 10
        ];
        // 1.9.4.1 = 1779000000 (0x6A0BB9C0 -> varint: C0 B3 AF D0 06)
        let ts_inner = vec![0x08, 0xC0, 0xB3, 0xAF, 0xD0, 0x06];
        let ts_field9 = vec![0x22, ts_inner.len() as u8];
        let mut field1_payload = Vec::new();
        field1_payload.push(0x22); // tag for 1.4
        field1_payload.push(tokens_bytes.len() as u8);
        field1_payload.extend_from_slice(&tokens_bytes);
        field1_payload.push(0x4A); // tag for 1.9
        field1_payload.push((ts_field9.len() + ts_inner.len()) as u8);
        field1_payload.extend_from_slice(&ts_field9);
        field1_payload.extend_from_slice(&ts_inner);

        let mut blob = Vec::new();
        blob.push(0x0A); // tag for 1
        blob.push(field1_payload.len() as u8);
        blob.extend_from_slice(&field1_payload);

        let mut statement = connection
            .prepare("INSERT INTO gen_metadata (idx, data) VALUES (1, ?)")
            .unwrap();
        statement.bind((1, blob.as_slice())).unwrap();
        statement.next().unwrap();

        let _env_guard = super::super::GeminiDataDirEnvGuard::set(fixture.root());
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries(&shared, &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id.as_ref(), "session-db");
        assert_eq!(entries[0].data.message.usage.input_tokens, 100);
        assert_eq!(entries[0].data.message.usage.output_tokens, 50);
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 20);
        assert_eq!(entries[0].extra_total_tokens, 10);
    }
}
