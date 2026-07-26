use std::{
    fs,
    path::{Path, PathBuf},
    thread,
};

use ccusage_core::{LoadedEntry, cli::SharedArgs};

pub mod jsonl;
pub mod report;

pub use report::print_table_for_agent;

pub fn collect_usage_files(dir: &Path, files: &mut Vec<PathBuf>) {
    collect_files_with_extension(dir, "jsonl", files);
}

pub fn collect_files_with_extension(dir: &Path, extension: &str, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.filter_map(std::result::Result::ok) {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_file() && path.extension().is_some_and(|ext| ext == extension) {
            files.push(path);
        } else if file_type.is_dir() {
            collect_files_with_extension(&path, extension, files);
        }
    }
}

pub fn filter_loaded_entries_by_date(entries: &mut Vec<LoadedEntry>, shared: &SharedArgs) {
    if shared.since.is_none() && shared.until.is_none() {
        return;
    }
    entries.retain(|entry| {
        let date = entry.date.replace('-', "");
        shared.since.as_ref().is_none_or(|since| &date >= since)
            && shared.until.as_ref().is_none_or(|until| &date <= until)
    });
}

pub fn chunk_file_indexes_by_size(files: &[PathBuf], chunk_count: usize) -> Vec<Vec<usize>> {
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
    use super::read_files_parallel;
    use ccusage_test_support::Fixture;

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
    fn handles_empty_input() {
        let empty: Vec<std::path::PathBuf> = Vec::new();

        assert!(read_files_parallel(&empty, false, |_| 0_u8).is_empty());
    }
}
