use std::{
    path::{Path, PathBuf},
    thread,
};

pub(crate) mod all;
pub(crate) mod amp;
pub(crate) mod claude;
pub(crate) mod codebuff;
pub(crate) mod codex;
pub(crate) mod copilot;
pub(crate) mod droid;
pub(crate) mod gemini;
pub(crate) mod goose;
pub(crate) mod hermes;
pub(crate) mod jsonl;
pub(crate) mod kilo;
pub(crate) mod kimi;
pub(crate) mod openclaw;
pub(crate) mod opencode;
pub(crate) mod pi;
pub(crate) mod qwen;

/// Reads `files` by applying `read` to each path and returns the per-file
/// results in the **original file order**.
///
/// Reads run on a thread pool sized to [`std::thread::available_parallelism`],
/// except when `single_thread` is set or only a single worker would be used, in
/// which case they run sequentially. Files are balanced across workers by byte
/// size via [`chunk_file_indexes_by_size`](crate::chunk_file_indexes_by_size)
/// so a few large files do not serialize one worker, and results are reassembled
/// by their original index. Because the output order never depends on thread
/// scheduling, any sequential dedup the caller runs afterwards observes a
/// deterministic sequence regardless of how many workers ran.
///
/// The per-file `read` closure owns its error handling: it should return a
/// neutral value (typically an empty `Vec` or `None`) and log on failure so a
/// single unreadable file never aborts the whole load, mirroring the Claude
/// loader's swallow-and-continue behavior.
pub(crate) fn read_files_parallel<T, F>(files: &[PathBuf], single_thread: bool, read: F) -> Vec<T>
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

    let chunks = crate::chunk_file_indexes_by_size(files, worker_count);
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
        // Real files with widely varying sizes so the size-balanced chunker
        // spreads them across multiple workers, exercising the parallel path.
        let fixture = Fixture::new();
        let files = (0..256)
            .map(|index| {
                let body = "x".repeat((index % 17) * 64 + 1);
                fixture.write_file(format!("file-{index:03}.txt"), format!("{index}:{body}"))
            })
            .collect::<Vec<_>>();

        // Return the leading "<index>:" marker so we can assert the result order
        // matches the input order regardless of how workers were scheduled.
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
        let result = read_files_parallel(&empty, false, |_| 0_u8);
        assert!(result.is_empty());
    }
}
