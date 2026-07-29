use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
};

use jiff::tz::TimeZone as JiffTimeZone;

use crate::{
    LoadedEntry, PricingMap, Result, cli::CostMode, cli::SharedArgs, debug_log, parse_tz,
    read_files_parallel,
};

use super::{
    parser::{UsageRecord, record_to_entry},
    paths::antigravity_db_paths,
    proto::{ModelUsage, parse_generation_metadata, parse_step_metadata},
};

/// Every conversation step, in the order Antigravity appended them.
const ANTIGRAVITY_STEP_QUERY: &str = r#"
SELECT idx, metadata
FROM steps
WHERE metadata IS NOT NULL
ORDER BY idx
"#;

/// Per-generation metadata, which is the only source of model names.
const ANTIGRAVITY_GENERATION_QUERY: &str = r#"
SELECT idx, data
FROM gen_metadata
WHERE data IS NOT NULL
ORDER BY idx
"#;

pub fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent("Antigravity"),
        shared.json,
        || load_entries_inner(shared, pricing),
    )
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let db_paths = antigravity_db_paths()?;
    let loaded = read_files_parallel(&db_paths, shared.single_thread, |db_path| {
        collect_records(db_path, shared)
    });
    Ok(records_to_entries(
        loaded,
        tz.as_ref(),
        shared.mode,
        pricing,
    ))
}

/// Turn the per-database records into entries, keeping each invocation once.
///
/// Groups arrive in the sorted path order path discovery fixed, so which record
/// survives for a given response id does not depend on how the parallel reads
/// happened to finish.
fn records_to_entries(
    groups: impl IntoIterator<Item = Vec<UsageRecord>>,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: &PricingMap,
) -> Vec<LoadedEntry> {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for records in groups {
        for record in records {
            // A record with no server-assigned identity cannot be matched against
            // anything, so it is kept rather than guessed at.
            if let Some(identity) = record.usage.identity()
                && !seen.insert(identity.to_string())
            {
                continue;
            }
            if let Some(entry) = record_to_entry(record, tz, mode, pricing) {
                entries.push(entry);
            }
        }
    }
    entries.sort_by_key(|entry| entry.timestamp);
    entries
}

/// Read every blob column of one conversation database.
fn read_blobs(db_path: &Path, query: &str, shared: &SharedArgs) -> Vec<Vec<u8>> {
    let Ok(connection) =
        sqlite::Connection::open_with_flags(db_path, sqlite::OpenFlags::new().with_read_only())
    else {
        debug_log(
            shared,
            format!(
                "Failed to open Antigravity conversation {}",
                db_path.display()
            ),
        );
        return Vec::new();
    };
    // A conversation database created by a different Antigravity version can be
    // missing a table entirely, which is a prepare-time error rather than a reason
    // to discard the rest of the conversation.
    let Ok(mut statement) = connection.prepare(query) else {
        debug_log(
            shared,
            format!(
                "Failed to query Antigravity conversation {}",
                db_path.display()
            ),
        );
        return Vec::new();
    };

    let mut blobs = Vec::new();
    loop {
        match statement.next() {
            Ok(sqlite::State::Row) => {
                if let Ok(blob) = statement.read::<Vec<u8>, _>(1) {
                    blobs.push(blob);
                }
            }
            Ok(sqlite::State::Done) => break,
            Err(_) => {
                debug_log(
                    shared,
                    format!(
                        "Failed to read Antigravity conversation {}",
                        db_path.display()
                    ),
                );
                break;
            }
        }
    }
    blobs
}

/// Collect every model invocation recorded by one conversation database.
///
/// `steps` is the primary source: it holds one row per invocation, including the
/// background calls that `gen_metadata` never records. `gen_metadata` is read for
/// two reasons — it is the only place a model name appears, and it keeps usage
/// visible for a step that has since been pruned.
fn collect_records(db_path: &Path, shared: &SharedArgs) -> Vec<UsageRecord> {
    let conversation_id: Arc<str> = db_path.file_stem().map_or_else(
        || Arc::from("antigravity"),
        |stem| Arc::from(stem.to_string_lossy().as_ref()),
    );

    // Names first, so both sources can be attributed in one pass.
    let generations = read_blobs(db_path, ANTIGRAVITY_GENERATION_QUERY, shared)
        .iter()
        .flat_map(|blob| parse_generation_metadata(blob))
        .collect::<Vec<_>>();
    let mut models = HashMap::new();
    for generation in &generations {
        let Some(model) = generation.model.as_deref() else {
            continue;
        };
        for usage in &generation.usages {
            if let Some(identity) = usage.identity() {
                models.insert(identity.to_string(), model.to_string());
            }
        }
    }

    let name_of = |usage: &ModelUsage| {
        usage
            .identity()
            .and_then(|identity| models.get(identity))
            .cloned()
    };

    let mut records = Vec::new();
    for blob in read_blobs(db_path, ANTIGRAVITY_STEP_QUERY, shared) {
        let step = parse_step_metadata(&blob);
        let Some(timestamp) = step.timestamp else {
            // A usage row with no timestamp cannot be placed in any reporting
            // period, so there is nothing useful to do with it.
            continue;
        };
        for usage in step.usages {
            let model = name_of(&usage);
            records.push(UsageRecord {
                conversation_id: Arc::clone(&conversation_id),
                timestamp,
                usage,
                model,
            });
        }
    }

    for generation in generations {
        let Some(timestamp) = generation.timestamp else {
            continue;
        };
        for usage in generation.usages {
            // Only identifiable rows are taken from `gen_metadata`. Without an
            // identity there is no way to tell a pruned step's usage apart from a
            // row already collected above, and counting it twice is worse than
            // missing it.
            if usage.identity().is_none() {
                continue;
            }
            let model = name_of(&usage);
            records.push(UsageRecord {
                conversation_id: Arc::clone(&conversation_id),
                timestamp,
                usage,
                model,
            });
        }
    }
    records
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::proto::encode::{field_bytes, field_varint, timestamp_bytes, usage_bytes};
    use ccusage_test_support::fs_fixture;

    /// Build a `CortexStepMetadata` blob carrying one invocation.
    fn step_blob(seconds: i64, usage: &[u8]) -> Vec<u8> {
        let mut blob = field_bytes(1, &timestamp_bytes(seconds, 0));
        blob.extend(field_bytes(9, usage));
        blob
    }

    /// Build a `gen_metadata` blob naming one invocation.
    fn generation_blob(seconds: i64, model: &str, usage: &[u8]) -> Vec<u8> {
        let mut chat = field_bytes(4, usage);
        chat.extend(field_bytes(
            9,
            &field_bytes(4, &timestamp_bytes(seconds, 0)),
        ));
        chat.extend(field_bytes(19, model.as_bytes()));
        field_bytes(1, &chat)
    }

    fn create_conversation_db(path: &Path) {
        let db = sqlite::open(path).unwrap();
        db.execute(
            r#"
CREATE TABLE steps (idx INTEGER PRIMARY KEY, metadata BLOB);
CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB);
"#,
        )
        .unwrap();
    }

    fn insert_blob(path: &Path, table: &str, idx: i64, blob: &[u8]) {
        let db = sqlite::open(path).unwrap();
        let column = if table == "steps" { "metadata" } else { "data" };
        let mut statement = db
            .prepare(format!(
                "INSERT INTO {table} (idx, {column}) VALUES (?1, ?2)"
            ))
            .unwrap();
        statement.bind((1, idx)).unwrap();
        statement.bind((2, blob)).unwrap();
        statement.next().unwrap();
    }

    /// A conversation with one named invocation that used a cache read.
    fn seed_conversation(path: &Path) {
        create_conversation_db(path);
        let usage = usage_bytes(1071, 4050, 375, 16275, "response-a");
        insert_blob(path, "steps", 0, &step_blob(1_785_328_986, &usage));
        insert_blob(
            path,
            "gen_metadata",
            0,
            &generation_blob(1_785_328_986, "gemini-3.6-flash", &usage),
        );
    }

    fn load(db_paths: &[PathBuf]) -> Vec<LoadedEntry> {
        let shared = SharedArgs::default();
        let pricing = PricingMap::load_embedded();
        let groups = db_paths
            .iter()
            .map(|db_path| collect_records(db_path, &shared))
            .collect::<Vec<_>>();
        records_to_entries(
            groups,
            Some(&jiff::tz::TimeZone::UTC),
            shared.mode,
            &pricing,
        )
    }

    #[test]
    fn names_a_step_invocation_from_its_generation_row() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("adf2fd49.db");
        seed_conversation(&db_path);

        let entries = load(&[db_path]);

        // The same invocation is recorded by both tables and must land once.
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("gemini-3.6-flash"));
        assert_eq!(entries[0].session_id.as_ref(), "adf2fd49");
        assert_eq!(entries[0].data.message.usage.input_tokens, 4050);
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 16275);
    }

    #[test]
    fn prices_an_invocation_its_generation_row_named() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("adf2fd49.db");
        create_conversation_db(&db_path);
        // Anthropic rates are embedded, so this exercises the cost path offline.
        let usage = usage_bytes(1071, 4050, 375, 0, "response-a");
        insert_blob(&db_path, "steps", 0, &step_blob(1_785_328_986, &usage));
        insert_blob(
            &db_path,
            "gen_metadata",
            0,
            &generation_blob(1_785_328_986, "claude-sonnet-4-5", &usage),
        );

        let entries = load(&[db_path]);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("claude-sonnet-4-5"));
        assert!(entries[0].cost > 0.0);
        assert_eq!(entries[0].missing_pricing_model, None);
    }

    #[test]
    fn counts_a_retry_attempt_without_recounting_the_successful_one() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("adf2fd49.db");
        create_conversation_db(&db_path);
        let succeeded = usage_bytes(1071, 4050, 375, 0, "response-ok");
        // `retry_infos` repeats the successful attempt alongside the failed one,
        // so both shapes have to survive without inflating the total.
        let mut blob = step_blob(1_785_328_986, &succeeded);
        blob.extend(field_bytes(28, &field_bytes(2, &succeeded)));
        blob.extend(field_bytes(
            28,
            &field_bytes(2, &usage_bytes(1071, 4000, 0, 0, "response-failed")),
        ));
        insert_blob(&db_path, "steps", 0, &blob);

        let entries = load(&[db_path]);

        assert_eq!(entries.len(), 2);
        let input = entries
            .iter()
            .map(|entry| entry.data.message.usage.input_tokens)
            .sum::<u64>();
        assert_eq!(input, 8050);
    }

    #[test]
    fn counts_a_sub_conversation_once_when_its_parent_repeats_it() {
        let fixture = fs_fixture!({});
        let parent = fixture.path("parent.db");
        let child = fixture.path("child.db");
        seed_conversation(&parent);
        seed_conversation(&child);

        let entries = load(&[parent, child]);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id.as_ref(), "parent");
    }

    #[test]
    fn keeps_usage_from_a_generation_whose_step_was_pruned() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("adf2fd49.db");
        create_conversation_db(&db_path);
        insert_blob(
            &db_path,
            "gen_metadata",
            0,
            &generation_blob(
                1_785_328_986,
                "gemini-3.6-flash",
                &usage_bytes(1071, 20110, 33, 0, "response-a"),
            ),
        );

        let entries = load(&[db_path]);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.input_tokens, 20110);
    }

    #[test]
    fn keeps_a_background_call_that_no_generation_row_names() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("adf2fd49.db");
        create_conversation_db(&db_path);
        insert_blob(
            &db_path,
            "steps",
            0,
            &step_blob(1_785_328_981, &usage_bytes(1050, 93, 3, 0, "response-b")),
        );

        let entries = load(&[db_path]);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("antigravity-model-1050"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 93);
        assert_eq!(
            entries[0].missing_pricing_model.as_deref(),
            Some("antigravity-model-1050")
        );
    }

    #[test]
    fn skips_a_step_that_records_no_timestamp() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("adf2fd49.db");
        create_conversation_db(&db_path);
        insert_blob(
            &db_path,
            "steps",
            0,
            &field_bytes(9, &usage_bytes(1071, 4050, 375, 0, "response-a")),
        );

        assert!(load(&[db_path]).is_empty());
    }

    #[test]
    fn tolerates_a_conversation_database_without_the_expected_tables() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("adf2fd49.db");
        sqlite::open(&db_path)
            .unwrap()
            .execute("CREATE TABLE unrelated (idx INTEGER PRIMARY KEY)")
            .unwrap();

        assert!(load(&[db_path]).is_empty());
    }

    #[test]
    fn ignores_a_step_holding_an_undecodable_blob() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("adf2fd49.db");
        create_conversation_db(&db_path);
        insert_blob(&db_path, "steps", 0, &[0xff, 0xff, 0xff, 0xff]);
        insert_blob(
            &db_path,
            "steps",
            1,
            &step_blob(
                1_785_328_986,
                &usage_bytes(1071, 4050, 375, 0, "response-a"),
            ),
        );

        // One corrupt row must not cost the rest of the conversation.
        let entries = load(&[db_path]);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.usage.input_tokens, 4050);
    }

    #[test]
    fn ignores_a_step_whose_usage_carries_no_tokens() {
        let fixture = fs_fixture!({});
        let db_path = fixture.path("adf2fd49.db");
        create_conversation_db(&db_path);
        let mut usage = field_varint(1, 1071);
        usage.extend(field_bytes(11, b"response-a"));
        insert_blob(&db_path, "steps", 0, &step_blob(1_785_328_986, &usage));

        assert!(load(&[db_path]).is_empty());
    }
}
