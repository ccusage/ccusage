use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use jiff::tz::TimeZone as JiffTimeZone;

use super::{
    parser::{self, TrajectoryMetadata},
    paths,
};
use ccusage_adapter_common::read_files_parallel;
use ccusage_core::cli::{CostMode, SharedArgs};
use ccusage_core::*;

pub fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent("Antigravity"),
        shared.json,
        || load_entries_inner(shared, pricing),
    )
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let roots = paths::paths()?;
    // Fast detection: without Antigravity data roots there is nothing to do.
    if roots.is_empty() {
        return Ok(Vec::new());
    }
    load_entries_from_roots(&roots, shared, pricing)
}

pub(super) fn load_entries_from_roots(
    roots: &[PathBuf],
    shared: &SharedArgs,
    pricing: &PricingMap,
) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let mut db_paths = Vec::new();
    for root in roots {
        db_paths.extend(paths::conversation_db_paths(root));
    }
    // Open each database in parallel (a fresh read-only connection per DB),
    // then run the sequential responseId dedup over the original path order so
    // the surviving record per id matches the single-threaded read.
    let loaded = read_files_parallel(&db_paths, shared.single_thread, |db_path| {
        load_entries_from_database(db_path, tz.as_ref(), shared.mode, Some(pricing), shared)
    });
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for db_entries in loaded {
        for entry in db_entries {
            if let Some(id) = entry.data.message.id.as_deref().filter(|id| !id.is_empty())
                && !seen.insert(id.to_string())
            {
                continue;
            }
            entries.push(entry);
        }
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

fn load_entries_from_database(
    db_path: &Path,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: Option<&PricingMap>,
    shared: &SharedArgs,
) -> Vec<LoadedEntry> {
    let Ok(connection) =
        sqlite::Connection::open_with_flags(db_path, sqlite::OpenFlags::new().with_read_only())
    else {
        debug_log(
            shared,
            format!("Failed to open Antigravity database: {}", db_path.display()),
        );
        return Vec::new();
    };
    // A database without gen_metadata is not an Antigravity conversation DB.
    let Ok(mut statement) = connection.prepare("SELECT data FROM gen_metadata ORDER BY idx") else {
        return Vec::new();
    };
    let trajectory = read_trajectory_metadata(&connection);
    let context = parser::session_context(db_path, &trajectory, file_mtime_ms(db_path));

    let mut entries = Vec::new();
    loop {
        match statement.next() {
            Ok(sqlite::State::Row) => {
                let Ok(blob) = statement.read::<Vec<u8>, _>(0) else {
                    continue;
                };
                let record = parser::decode_generation(&blob);
                if let Some(entry) =
                    parser::generation_to_entry(&record, &context, tz, mode, pricing)
                {
                    entries.push(entry);
                }
            }
            Ok(sqlite::State::Done) => break,
            Err(_) => {
                debug_log(
                    shared,
                    format!(
                        "Failed to query Antigravity database: {}",
                        db_path.display()
                    ),
                );
                break;
            }
        }
    }
    entries
}

fn read_trajectory_metadata(connection: &sqlite::Connection) -> TrajectoryMetadata {
    let Ok(mut statement) = connection.prepare("SELECT data FROM trajectory_metadata_blob LIMIT 1")
    else {
        return TrajectoryMetadata::default();
    };
    if let Ok(sqlite::State::Row) = statement.next()
        && let Ok(blob) = statement.read::<Vec<u8>, _>(0)
    {
        return parser::decode_trajectory_metadata(&blob);
    }
    TrajectoryMetadata::default()
}

fn file_mtime_ms(db_path: &Path) -> Option<i64> {
    let modified = fs::metadata(db_path).ok()?.modified().ok()?;
    let millis = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    i64::try_from(millis).ok()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{super::parser::encode, *};
    use crate::cli::SharedArgs;
    use ccusage_test_support::fs_fixture;

    fn create_conversation_db(path: &Path, generations: &[Vec<u8>], trajectory: Option<&[u8]>) {
        let db = sqlite::open(path).unwrap();
        db.execute("CREATE TABLE gen_metadata (idx INTEGER, data BLOB)")
            .unwrap();
        let mut statement = db
            .prepare("INSERT INTO gen_metadata (idx, data) VALUES (?1, ?2)")
            .unwrap();
        for (index, blob) in generations.iter().enumerate() {
            statement.bind((1, index as i64)).unwrap();
            statement.bind((2, blob.as_slice())).unwrap();
            statement.next().unwrap();
            statement.reset().unwrap();
        }
        if let Some(trajectory) = trajectory {
            db.execute("CREATE TABLE trajectory_metadata_blob (data BLOB)")
                .unwrap();
            let mut statement = db
                .prepare("INSERT INTO trajectory_metadata_blob (data) VALUES (?1)")
                .unwrap();
            statement.bind((1, trajectory)).unwrap();
            statement.next().unwrap();
        }
    }

    fn generation(
        model: &'static str,
        response_id: &'static str,
        input: u64,
        output: u64,
    ) -> Vec<u8> {
        encode::generation_blob(&encode::Generation {
            model,
            timestamp: Some((1_767_312_000, 0)),
            system_input: 1000,
            fresh_input: input,
            cache_read: 10,
            output,
            thinking: 5,
            response_id,
        })
    }

    fn load(roots: &[PathBuf]) -> Vec<LoadedEntry> {
        let shared = SharedArgs {
            mode: CostMode::Display,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        load_entries_from_roots(roots, &shared, &PricingMap::default()).unwrap()
    }

    #[test]
    fn loads_generation_entries_from_conversation_db() {
        let fixture = fs_fixture!({});
        let root = fixture.create_dir_all("antigravity/conversations");
        create_conversation_db(
            &root.join("conv-1.db"),
            &[generation("gemini-3.1-pro-low", "resp-1", 6321, 604)],
            Some(&encode::trajectory_blob(
                Some((1_767_312_000, 0)),
                "file:///Users/test/My%20Projects/app",
            )),
        );

        let entries = load(&[fixture.path("antigravity")]);

        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.session_id.as_ref(), "conv-1");
        assert_eq!(entry.project.as_ref(), "app");
        assert_eq!(entry.project_path.as_ref(), "/Users/test/My Projects/app");
        assert_eq!(entry.model.as_deref(), Some("gemini-3.1-pro"));
        assert_eq!(entry.data.message.id.as_deref(), Some("resp-1"));
        assert_eq!(entry.data.message.usage.input_tokens, 1000 + 6321);
        assert_eq!(entry.data.message.usage.output_tokens, 604);
        assert_eq!(entry.data.message.usage.cache_read_input_tokens, 10);
        assert_eq!(entry.extra_total_tokens, 5);
        assert_eq!(entry.date, "2026-01-02");
    }

    #[test]
    fn dedupes_response_ids_within_and_across_dbs() {
        let fixture = fs_fixture!({});
        let root_a = fixture.create_dir_all("antigravity/conversations");
        let root_b = fixture.create_dir_all("antigravity-cli/conversations");
        create_conversation_db(
            &root_a.join("conv-a.db"),
            &[
                generation("gemini-3.1-pro-low", "resp-1", 100, 1),
                generation("gemini-3.1-pro-low", "resp-1", 999, 9),
                generation("gemini-3.1-pro-low", "resp-2", 200, 2),
            ],
            None,
        );
        create_conversation_db(
            &root_b.join("conv-b.db"),
            &[
                generation("gemini-pro-default", "resp-2", 888, 8),
                generation("gemini-pro-default", "resp-3", 300, 3),
            ],
            None,
        );

        let entries = load(&[fixture.path("antigravity"), fixture.path("antigravity-cli")]);

        assert_eq!(entries.len(), 3);
        // First occurrence wins, both within one DB and across DBs.
        let tokens: Vec<u64> = entries
            .iter()
            .map(|entry| entry.data.message.usage.input_tokens)
            .collect();
        assert_eq!(tokens, vec![1100, 1200, 1300]);
    }

    #[test]
    fn keeps_generations_without_response_id() {
        let fixture = fs_fixture!({});
        let root = fixture.create_dir_all("antigravity/conversations");
        create_conversation_db(
            &root.join("conv-1.db"),
            &[
                generation("gemini-3.1-pro-low", "", 100, 1),
                generation("gemini-3.1-pro-low", "", 100, 1),
            ],
            None,
        );

        let entries = load(&[fixture.path("antigravity")]);

        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn skips_all_zero_generations() {
        let fixture = fs_fixture!({});
        let root = fixture.create_dir_all("antigravity/conversations");
        create_conversation_db(
            &root.join("conv-1.db"),
            &[
                encode::generation_blob(&encode::Generation {
                    model: "gemini-3.1-pro-low",
                    timestamp: Some((1_767_312_000, 0)),
                    system_input: 0,
                    fresh_input: 0,
                    cache_read: 0,
                    output: 0,
                    thinking: 0,
                    response_id: "resp-zero",
                }),
                generation("gemini-3.1-pro-low", "resp-1", 100, 1),
            ],
            None,
        );

        let entries = load(&[fixture.path("antigravity")]);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].data.message.id.as_deref(), Some("resp-1"));
    }

    #[test]
    fn ignores_databases_without_gen_metadata() {
        let fixture = fs_fixture!({});
        let root = fixture.create_dir_all("antigravity/conversations");
        let db = sqlite::open(root.join("other.db")).unwrap();
        db.execute("CREATE TABLE steps (idx INTEGER, data BLOB)")
            .unwrap();

        let entries = load(&[fixture.path("antigravity")]);

        assert!(entries.is_empty());
    }

    #[test]
    fn falls_back_to_trajectory_created_at_then_file_mtime() {
        let fixture = fs_fixture!({});
        let root = fixture.create_dir_all("antigravity/conversations");
        let no_timestamp = |response_id: &'static str| {
            encode::generation_blob(&encode::Generation {
                model: "gemini-3.1-pro-low",
                timestamp: None,
                system_input: 1000,
                fresh_input: 100,
                cache_read: 0,
                output: 1,
                thinking: 0,
                response_id,
            })
        };
        create_conversation_db(
            &root.join("conv-with-trajectory.db"),
            &[no_timestamp("resp-1")],
            Some(&encode::trajectory_blob(
                Some((1_767_312_000, 0)),
                "file:///Users/test/app",
            )),
        );
        create_conversation_db(&root.join("conv-mtime.db"), &[no_timestamp("resp-2")], None);

        let entries = load(&[fixture.path("antigravity")]);

        assert_eq!(entries.len(), 2);
        let with_trajectory = entries
            .iter()
            .find(|entry| entry.session_id.as_ref() == "conv-with-trajectory")
            .unwrap();
        assert_eq!(
            with_trajectory.timestamp,
            crate::TimestampMs::from_millis(1_767_312_000_000)
        );
        let with_mtime = entries
            .iter()
            .find(|entry| entry.session_id.as_ref() == "conv-mtime")
            .unwrap();
        let mtime_ms = i64::try_from(
            std::fs::metadata(root.join("conv-mtime.db"))
                .unwrap()
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap();
        assert_eq!(
            with_mtime.timestamp,
            crate::TimestampMs::from_millis(mtime_ms)
        );
    }

    #[test]
    fn discovers_default_roots_under_home() {
        let fixture = fs_fixture!({
            ".gemini/antigravity/conversations/.keep": "",
            ".gemini/antigravity-cli/conversations/.keep": "",
            ".gemini/antigravity-ide/conversations/.keep": "",
        });
        let _env = ccusage_test_support::EnvVarsGuard::set_many([
            ("HOME", Some(fixture.root().as_os_str().to_os_string())),
            (paths::ANTIGRAVITY_DATA_DIR_ENV, None),
        ]);

        let roots = paths::paths().unwrap();

        assert_eq!(
            roots,
            vec![
                fixture.path(".gemini/antigravity"),
                fixture.path(".gemini/antigravity-cli"),
                fixture.path(".gemini/antigravity-ide"),
            ]
        );
    }
}
