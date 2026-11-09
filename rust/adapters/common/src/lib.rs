use std::{
    fs,
    path::{Path, PathBuf},
    thread,
};

use ccusage_core::{
    LoadedEntry, UsageSummary, cli::SharedArgs, date_within_range, format_date_tz,
    parse_ts_timestamp, parse_tz,
};

pub mod jsonl;
pub mod report;

pub use report::print_table_for_agent;

pub fn collect_usage_files(dir: &Path, files: &mut Vec<PathBuf>) {
    collect_files_with_extension(dir, "jsonl", files);
}

pub fn collect_files_with_extension(dir: &Path, extension: &str, files: &mut Vec<PathBuf>) {
    collect_files_matching(
        dir,
        &|path| path.extension().is_some_and(|ext| ext == extension),
        files,
    );
}

pub fn collect_files_matching(
    dir: &Path,
    matches: &impl Fn(&Path) -> bool,
    files: &mut Vec<PathBuf>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.filter_map(std::result::Result::ok) {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_file() && matches(&path) {
            files.push(path);
        } else if file_type.is_dir() {
            collect_files_matching(&path, matches, files);
        }
    }
}

pub fn filter_loaded_entries_by_date(entries: &mut Vec<LoadedEntry>, shared: &SharedArgs) {
    if shared.since.is_none() && shared.until.is_none() {
        return;
    }
    entries.retain(|entry| {
        date_within_range(
            &entry.date,
            shared.since.as_deref(),
            shared.until.as_deref(),
        )
    });
}

/// Keeps the sessions whose last activity falls on a day inside the
/// `--since`/`--until` window, with the day taken in the report timezone the
/// same way `LoadedEntry::date` is.
pub fn filter_session_summaries_by_last_activity(
    rows: &mut Vec<UsageSummary>,
    shared: &SharedArgs,
) {
    if shared.since.is_none() && shared.until.is_none() {
        return;
    }
    let timezone = parse_tz(shared.timezone.as_deref());
    rows.retain(|row| {
        row.last_activity
            .as_deref()
            .and_then(parse_ts_timestamp)
            .is_some_and(|timestamp| {
                date_within_range(
                    &format_date_tz(timestamp, timezone.as_ref()),
                    shared.since.as_deref(),
                    shared.until.as_deref(),
                )
            })
    });
}

pub fn chunk_file_indexes_by_size(files: &[PathBuf], chunk_count: usize) -> Vec<Vec<usize>> {
    // Callers derive the count from available_parallelism, which can be 0 for a
    // caller that clamps against an empty file list; one chunk still returns
    // every index rather than indexing an empty vector.
    let chunk_count = chunk_count.max(1);
    let mut weighted_indexes = Vec::with_capacity(files.len());
    for (index, file) in files.iter().enumerate() {
        let size = fs::metadata(file).map_or(0, |metadata| metadata.len());
        weighted_indexes.push((index, size));
    }
    weighted_indexes.sort_unstable_by(|a, b| match b.1.cmp(&a.1) {
        std::cmp::Ordering::Equal => a.0.cmp(&b.0),
        order => order,
    });

    let mut chunks = vec![Vec::new(); chunk_count];
    let mut chunk_sizes = vec![0_u64; chunk_count];
    for (index, size) in weighted_indexes {
        let mut target = 0;
        for candidate in 1..chunk_sizes.len() {
            if chunk_sizes[candidate] < chunk_sizes[target] {
                target = candidate;
            }
        }
        chunks[target].push(index);
        chunk_sizes[target] = chunk_sizes[target].saturating_add(size);
    }

    chunks
        .into_iter()
        .filter(|chunk| !chunk.is_empty())
        .collect()
}

/// Reads `files` by applying `read` to each path and returns results in file order.
pub fn read_files_parallel<T, F>(files: &[PathBuf], single_thread: bool, read: F) -> Vec<T>
where
    T: Send,
    F: Fn(&Path) -> T + Sync,
{
    let worker_count = if single_thread {
        1
    } else {
        thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .min(files.len())
    };
    if worker_count <= 1 {
        return files.iter().map(|file| read(file.as_path())).collect();
    }

    let chunks = chunk_file_indexes_by_size(files, worker_count);
    let read = &read;
    thread::scope(|scope| {
        let mut handles = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            handles.push(scope.spawn(move || {
                chunk
                    .into_iter()
                    .map(|index| (index, read(files[index].as_path())))
                    .collect::<Vec<_>>()
            }));
        }
        let mut results: Vec<Option<T>> = Vec::with_capacity(files.len());
        results.resize_with(files.len(), || None);
        for (index, value) in handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("file read worker panicked"))
        {
            results[index] = Some(value);
        }
        results
            .into_iter()
            .map(|value| value.expect("file read worker returned every file"))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use ccusage_core::{UsageSummary, cli::SharedArgs};
    use ccusage_test_support::Fixture;

    use super::{
        chunk_file_indexes_by_size, collect_files_matching,
        filter_session_summaries_by_last_activity, read_files_parallel,
    };

    fn session_summary(session_id: &str, last_activity: &str) -> UsageSummary {
        UsageSummary {
            date: None,
            month: None,
            week: None,
            session_id: Some(session_id.to_string()),
            project_path: None,
            last_activity: Some(last_activity.to_string()),
            first_activity: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            extra_total_tokens: 0,
            total_cost: 0.0,
            credits: None,
            message_count: None,
            models_used: Vec::new(),
            model_breakdowns: Vec::new(),
            project: None,
            versions: None,
        }
    }

    fn filtered_session_ids(shared: &SharedArgs) -> Vec<String> {
        let mut rows = vec![
            session_summary("before", "2026-02-22T23:59:59.000Z"),
            session_summary("start-of-day", "2026-02-23T00:00:00.000Z"),
            session_summary("end-of-day", "2026-02-23T23:59:59.999Z"),
            session_summary("after", "2026-02-24T00:00:00.000Z"),
        ];
        filter_session_summaries_by_last_activity(&mut rows, shared);
        rows.into_iter().filter_map(|row| row.session_id).collect()
    }

    #[test]
    fn session_window_bounds_are_inclusive_of_the_whole_day() {
        let shared = SharedArgs {
            since: Some("20260223".to_string()),
            until: Some("20260223".to_string()),
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        assert_eq!(
            filtered_session_ids(&shared),
            ["start-of-day", "end-of-day"]
        );
    }

    #[test]
    fn session_window_takes_the_day_in_the_report_timezone() {
        // Asia/Tokyo is UTC+9, so the window shifts nine hours earlier in UTC.
        let shared = SharedArgs {
            since: Some("20260223".to_string()),
            until: Some("20260223".to_string()),
            timezone: Some("Asia/Tokyo".to_string()),
            ..SharedArgs::default()
        };

        assert_eq!(filtered_session_ids(&shared), ["before", "start-of-day"]);
    }

    #[test]
    fn session_window_with_only_until_keeps_everything_through_that_day() {
        let shared = SharedArgs {
            until: Some("20260223".to_string()),
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        assert_eq!(
            filtered_session_ids(&shared),
            ["before", "start-of-day", "end-of-day"]
        );
    }

    #[test]
    fn session_window_with_only_since_keeps_everything_from_that_day() {
        let shared = SharedArgs {
            since: Some("20260223".to_string()),
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        assert_eq!(
            filtered_session_ids(&shared),
            ["start-of-day", "end-of-day", "after"]
        );
    }

    #[test]
    fn session_window_drops_rows_without_a_parseable_last_activity() {
        let missing = UsageSummary {
            last_activity: None,
            ..session_summary("missing", "")
        };
        let unparsable = session_summary("unparsable", "2026-02-23");
        for shared in [
            SharedArgs {
                since: Some("20260101".to_string()),
                ..SharedArgs::default()
            },
            SharedArgs {
                until: Some("20991231".to_string()),
                ..SharedArgs::default()
            },
        ] {
            let mut rows = vec![missing.clone(), unparsable.clone()];
            filter_session_summaries_by_last_activity(&mut rows, &shared);
            assert!(rows.is_empty(), "{:?}/{:?}", shared.since, shared.until);
        }
    }

    #[test]
    fn collects_nested_files_accepted_by_the_predicate() {
        let fixture = Fixture::new();
        let kept = fixture.write_file("a/b/kept.jsonl", "");
        let _ = fixture.write_file("a/skipped.txt", "");
        let mut files = Vec::new();

        collect_files_matching(
            fixture.root(),
            &|path| path.extension().is_some_and(|ext| ext == "jsonl"),
            &mut files,
        );

        assert_eq!(files, vec![kept]);
    }

    #[test]
    fn preserves_file_order_and_matches_single_thread() {
        let fixture = Fixture::new();
        let files = (0..256)
            .map(|index| {
                let body = "x".repeat((index % 17) * 64 + 1);
                fixture.write_file(format!("file-{index:03}.txt"), format!("{index}:{body}"))
            })
            .collect::<Vec<_>>();
        let read = |path: &std::path::Path| {
            let content = std::fs::read_to_string(path).unwrap();
            content.split(':').next().unwrap().to_string()
        };

        let single = read_files_parallel(&files, true, read);
        let multi = read_files_parallel(&files, false, read);
        let expected = (0..256).map(|index| index.to_string()).collect::<Vec<_>>();

        assert_eq!(single, expected);
        assert_eq!(multi, expected);
    }

    #[test]
    fn treats_a_zero_chunk_count_as_one_chunk() {
        let fixture = Fixture::new();
        let files = vec![fixture.write_file("only.txt", "body")];

        assert_eq!(chunk_file_indexes_by_size(&files, 0), vec![vec![0]]);
    }

    #[test]
    fn handles_empty_input() {
        let empty: Vec<std::path::PathBuf> = Vec::new();

        assert!(read_files_parallel(&empty, false, |_| 0_u8).is_empty());
    }
}
