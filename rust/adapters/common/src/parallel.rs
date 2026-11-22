use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
    thread,
};

use ccusage_core::parallel::spawn_scoped_worker;

pub fn chunk_file_indexes_by_size(files: &[PathBuf], chunk_count: usize) -> Vec<Vec<usize>> {
    let chunk_count = chunk_count.max(1);
    chunk_indexes_by_size(files, PathBuf::as_path, chunk_count)
}

fn chunk_indexes_by_size<I>(
    items: &[I],
    path: fn(&I) -> &Path,
    chunk_count: usize,
) -> Vec<Vec<usize>> {
    let sizes = items
        .iter()
        .map(|item| fs::metadata(path(item)).map_or(0, |metadata| metadata.len()))
        .collect();
    balance_chunks(sizes, chunk_count)
}

fn balance_chunks(sizes: Vec<u64>, chunk_count: usize) -> Vec<Vec<usize>> {
    let mut weighted_indexes = sizes.into_iter().enumerate().collect::<Vec<_>>();
    weighted_indexes.sort_unstable_by(|a, b| (b.1, a.0).cmp(&(a.1, b.0)));

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

fn chunk_by_count(len: usize, workers: usize) -> Vec<Vec<usize>> {
    let chunk_size = len.div_ceil(workers);
    (0..len)
        .collect::<Vec<_>>()
        .chunks(chunk_size)
        .map(<[usize]>::to_vec)
        .collect()
}

pub fn worker_count(single_thread: bool, items: usize) -> usize {
    if single_thread || items <= 1 {
        return 1;
    }
    thread::available_parallelism()
        .map_or(1, usize::from)
        .min(items)
}

pub enum Split<I> {
    BySize(fn(&I) -> &Path),
    ByCount,
}

pub fn map_parallel<I, T, F>(items: &[I], single_thread: bool, split: Split<I>, map: F) -> Vec<T>
where
    I: Sync,
    T: Send,
    F: Fn(&I) -> T + Sync,
{
    let workers = worker_count(single_thread, items.len());
    if workers == 1 {
        return items.iter().map(map).collect();
    }
    let chunks = match split {
        Split::BySize(path) => chunk_indexes_by_size(items, path, workers),
        Split::ByCount => chunk_by_count(items.len(), workers),
    };
    let results = Mutex::new(Vec::with_capacity(items.len()));
    run_chunks(chunks, &|chunk| {
        let done = chunk
            .into_iter()
            .map(|index| (index, map(&items[index])))
            .collect::<Vec<_>>();
        results
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(done);
    });
    let mut results = results.into_inner().unwrap_or_else(PoisonError::into_inner);
    for slot in 0..results.len() {
        while results[slot].0 != slot {
            let target = results[slot].0;
            results.swap(slot, target);
        }
    }
    results.into_iter().map(|(_, result)| result).collect()
}

fn run_chunks(chunks: Vec<Vec<usize>>, work: &(dyn Fn(Vec<usize>) + Sync)) {
    thread::scope(|scope| {
        let workers = chunks
            .into_iter()
            .map(|chunk| spawn_scoped_worker(scope, move || work(chunk)))
            .collect::<Vec<_>>();
        for worker in workers {
            worker
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        }
    });
}

#[cfg(test)]
mod tests;
