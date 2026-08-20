use std::{collections::HashSet, fs, path::Path};

use crate::{LoadedEntry, PricingMap, Result, TimestampMs, cli::SharedArgs, read_files_parallel};

use super::{
    parser::{
        AntigravityEntry, DEFAULT_ANTIGRAVITY_MODEL, RawTokenStats, build_entry,
        parse_gen_metadata, parse_step_metadata, to_loaded_entry,
    },
    paths::antigravity_db_paths,
};

pub fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent("Antigravity"),
        shared.json,
        || load_entries_inner(shared, pricing),
    )
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let tz = crate::parse_tz(shared.timezone.as_deref());
    let db_paths = antigravity_db_paths()?;
    let loaded = read_files_parallel(&db_paths, shared.single_thread, |db_path| {
        load_conversation_db(db_path, shared)
    });

    let mut entries = Vec::new();
    let mut seen_sessions = HashSet::new();

    for db_entries in loaded {
        for entry in db_entries {
            if !seen_sessions.insert(entry.session_id.clone()) {
                // If we already saw this session from a prior path, skip
            }
            entries.push(to_loaded_entry(entry, tz.as_ref(), pricing));
        }
    }

    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

pub fn has_data() -> bool {
    antigravity_db_paths().is_ok_and(|files| !files.is_empty())
}

fn file_modified_timestamp(path: &Path) -> TimestampMs {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .map(TimestampMs::from_millis)
        .unwrap_or(TimestampMs::UNIX_EPOCH)
}

pub(super) fn load_conversation_db(db_path: &Path, shared: &SharedArgs) -> Vec<AntigravityEntry> {
    let Ok(connection) =
        sqlite::Connection::open_with_flags(db_path, sqlite::OpenFlags::new().with_read_only())
    else {
        crate::debug_log(
            shared,
            format!(
                "Failed to open Antigravity conversation database: {}",
                db_path.display()
            ),
        );
        return Vec::new();
    };

    let fallback_timestamp = file_modified_timestamp(db_path);
    let default_session_id = db_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let mut detected_model = None;
    let mut gen_stats_map = Vec::new();

    if let Ok(mut stmt) = connection.prepare("SELECT idx, data FROM gen_metadata ORDER BY idx ASC")
    {
        while let Ok(sqlite::State::Row) = stmt.next() {
            if let Ok(data) = stmt.read::<Vec<u8>, _>(1) {
                let (model_opt, stats) = parse_gen_metadata(&data);
                if let Some(m) = model_opt {
                    detected_model = Some(m);
                }
                if !stats.is_zero() {
                    gen_stats_map.push(stats);
                }
            }
        }
    }

    let model_name = detected_model.unwrap_or_else(|| DEFAULT_ANTIGRAVITY_MODEL.to_string());
    let mut entries = Vec::new();
    let mut step_count = 0u64;
    let mut accumulated_stats = RawTokenStats::default();
    let mut session_id = default_session_id;
    let mut earliest_timestamp = None;

    if let Ok(mut stmt) = connection
        .prepare("SELECT idx, metadata FROM steps WHERE metadata IS NOT NULL ORDER BY idx ASC")
    {
        while let Ok(sqlite::State::Row) = stmt.next() {
            step_count += 1;
            if let Ok(metadata) = stmt.read::<Vec<u8>, _>(1) {
                let (ts_opt, sess_opt, stats) = parse_step_metadata(&metadata);
                if let Some(s) = sess_opt {
                    session_id = s;
                }
                if ts_opt.is_some() && earliest_timestamp.is_none() {
                    earliest_timestamp = ts_opt;
                }
                if !stats.is_zero() {
                    accumulated_stats.input_tokens += stats.input_tokens;
                    accumulated_stats.output_tokens += stats.output_tokens;
                    accumulated_stats.cache_read_tokens += stats.cache_read_tokens;
                    accumulated_stats.reasoning_tokens += stats.reasoning_tokens;
                    accumulated_stats.generation_tokens += stats.generation_tokens;
                }
            }
        }
    }

    if accumulated_stats.is_zero() && !gen_stats_map.is_empty() {
        for stats in gen_stats_map {
            accumulated_stats.input_tokens += stats.input_tokens;
            accumulated_stats.output_tokens += stats.output_tokens;
            accumulated_stats.cache_read_tokens += stats.cache_read_tokens;
            accumulated_stats.reasoning_tokens += stats.reasoning_tokens;
            accumulated_stats.generation_tokens += stats.generation_tokens;
        }
    }

    if !accumulated_stats.is_zero() {
        let timestamp = earliest_timestamp.unwrap_or(fallback_timestamp);
        entries.push(build_entry(
            timestamp,
            session_id,
            model_name,
            accumulated_stats,
            step_count.max(1),
        ));
    }

    entries
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use ccusage_test_support::fs_fixture;

    fn create_antigravity_db(path: &Path) {
        let db = sqlite::open(path).unwrap();
        db.execute(
            "
                CREATE TABLE steps (
                    idx INTEGER PRIMARY KEY,
                    metadata BLOB
                );
                CREATE TABLE gen_metadata (
                    idx INTEGER PRIMARY KEY,
                    data BLOB
                );
            ",
        )
        .unwrap();
    }

    fn encode_varint(val: u64, buf: &mut Vec<u8>) {
        let mut v = val;
        while v >= 0x80 {
            buf.push(((v & 0x7F) as u8) | 0x80);
            v >>= 7;
        }
        buf.push((v & 0x7F) as u8);
    }

    fn encode_tag(field: u32, wire_type: u8, buf: &mut Vec<u8>) {
        let key = ((field as u64) << 3) | (wire_type as u64);
        encode_varint(key, buf);
    }

    fn encode_bytes_field(field: u32, data: &[u8], buf: &mut Vec<u8>) {
        encode_tag(field, 2, buf);
        encode_varint(data.len() as u64, buf);
        buf.extend_from_slice(data);
    }

    #[test]
    fn loads_entries_from_antigravity_db() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("session-123.db");
        create_antigravity_db(&db_path);

        // Build a mock gen_metadata blob with model "gemini-3.7-flash"
        let mut gen_blob = Vec::new();
        let mut gen_sub1 = Vec::new();
        encode_bytes_field(19, b"gemini-3.7-flash", &mut gen_sub1);
        encode_bytes_field(1, &gen_sub1, &mut gen_blob);

        // Build a mock step metadata blob with timestamp and token stats
        let mut step_blob = Vec::new();
        // Timestamp: seconds = 1750000000 (2025-06-15T15:06:40Z)
        let mut ts_sub = Vec::new();
        encode_tag(1, 0, &mut ts_sub);
        encode_varint(1750000000, &mut ts_sub);
        encode_bytes_field(1, &ts_sub, &mut step_blob);

        // Generation stats (field 9): input=1500, output=300, cache_read=200, reasoning=50
        let mut stats_sub = Vec::new();
        encode_tag(2, 0, &mut stats_sub);
        encode_varint(1500, &mut stats_sub);
        encode_tag(3, 0, &mut stats_sub);
        encode_varint(300, &mut stats_sub);
        encode_tag(5, 0, &mut stats_sub);
        encode_varint(200, &mut stats_sub);
        encode_tag(9, 0, &mut stats_sub);
        encode_varint(50, &mut stats_sub);
        encode_bytes_field(9, &stats_sub, &mut step_blob);

        let db = sqlite::open(&db_path).unwrap();
        let mut stmt1 = db
            .prepare("INSERT INTO gen_metadata (idx, data) VALUES (0, ?1)")
            .unwrap();
        stmt1.bind((1, gen_blob.as_slice())).unwrap();
        stmt1.next().unwrap();

        let mut stmt2 = db
            .prepare("INSERT INTO steps (idx, metadata) VALUES (0, ?1)")
            .unwrap();
        stmt2.bind((1, step_blob.as_slice())).unwrap();
        stmt2.next().unwrap();

        let pricing = PricingMap::load_embedded();
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let tz = crate::parse_tz(shared.timezone.as_deref());
        let entries = load_conversation_db(&db_path, &shared)
            .into_iter()
            .map(|e| to_loaded_entry(e, tz.as_ref(), &pricing))
            .collect::<Vec<_>>();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2025-06-15");
        assert_eq!(entries[0].session_id.as_ref(), "session-123");
        assert_eq!(entries[0].model.as_deref(), Some("gemini-3.7-flash"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 1500);
        assert_eq!(entries[0].data.message.usage.output_tokens, 300);
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 200);
        assert_eq!(entries[0].extra_total_tokens, 50);
        assert!(entries[0].cost > 0.0);
    }
}
