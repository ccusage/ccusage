use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Condvar, Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};

use ccusage_core::parallel::spawn_scoped_worker;

const PLAIN_PATH_BACKLOG: usize = 1024;

#[derive(Clone, Copy)]
enum ReadWorkload {
    PlainPaths,
    GroupedFiles,
}

pub struct DiscoveredFile {
    pub group: usize,
    pub index: usize,
    pub path: PathBuf,
}

pub type DiscoverFiles<'a> = dyn FnMut(&mut dyn FnMut(usize, PathBuf)) + 'a;

pub fn read_discovered_paths<T: Send>(
    single_thread: bool,
    mut discover: impl FnMut(&mut dyn FnMut(PathBuf)),
    read: impl Fn(&PathBuf) -> T + Sync,
) -> Vec<(PathBuf, T)> {
    collect_discovered(
        single_thread,
        ReadWorkload::PlainPaths,
        &mut |emit| discover(&mut |path| emit(0, path)),
        |file| {
            let result = read(&file.path);
            (file.path, result)
        },
    )
}

/// Reads files while `discover` emits them, possibly from several threads at once.
pub fn read_paths_discovered_in_parallel<T: Send>(
    mut discover: impl FnMut(&(dyn Fn(PathBuf) + Sync)),
    read: impl Fn(&PathBuf) -> T + Sync,
) -> Vec<(PathBuf, T)> {
    collect_in_parallel(
        ReadWorkload::PlainPaths,
        &mut |emit| discover(&|path| emit(0, path)),
        |file| {
            let result = read(&file.path);
            (file.path, result)
        },
    )
}

/// Reads files as discovery yields them. Parallel results have no ordering guarantee.
pub fn read_discovered_files<T: Send>(
    single_thread: bool,
    discover: &mut DiscoverFiles<'_>,
    read: impl Fn(&DiscoveredFile) -> T + Sync,
) -> Vec<(DiscoveredFile, T)> {
    collect_discovered(
        single_thread,
        ReadWorkload::GroupedFiles,
        discover,
        |file| {
            let result = read(&file);
            (file, result)
        },
    )
}

fn collect_discovered<T: Send>(
    single_thread: bool,
    workload: ReadWorkload,
    discover: &mut DiscoverFiles<'_>,
    read: impl Fn(DiscoveredFile) -> T + Sync,
) -> Vec<T> {
    if single_thread {
        let mut results = Vec::new();
        let mut index = 0;
        discover(&mut |group, path| {
            results.push(read(DiscoveredFile { group, index, path }));
            index += 1;
        });
        return results;
    }
    collect_in_parallel(
        workload,
        &mut |emit| discover(&mut |group, path| emit(group, path)),
        read,
    )
}

fn collect_in_parallel<T: Send>(
    workload: ReadWorkload,
    discover: &mut SharedDiscovery<'_>,
    read: impl Fn(DiscoveredFile) -> T + Sync,
) -> Vec<T> {
    let batches = Mutex::new(Vec::new());
    let consume = |files: &mut dyn Iterator<Item = DiscoveredFile>| {
        let results = files.map(&read).collect::<Vec<_>>();
        batches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(results);
    };
    read_discovered_in_parallel(workload, discover, &consume);
    let batches = batches.into_inner().unwrap_or_else(PoisonError::into_inner);
    let mut results = Vec::with_capacity(batches.iter().map(Vec::len).sum());
    for batch in batches {
        results.extend(batch);
    }
    results
}

type SharedDiscovery<'a> = dyn FnMut(&(dyn Fn(usize, PathBuf) + Sync)) + 'a;
type ConsumeFiles<'a> = dyn Fn(&mut dyn Iterator<Item = DiscoveredFile>) + Sync + 'a;

#[inline(never)]
fn read_discovered_in_parallel(
    workload: ReadWorkload,
    discover: &mut SharedDiscovery<'_>,
    consume: &ConsumeFiles<'_>,
) {
    let parallelism = thread::available_parallelism().map_or(1, usize::from);
    let (workers, capacity) = match workload {
        ReadWorkload::PlainPaths => (parallelism, PLAIN_PATH_BACKLOG),
        ReadWorkload::GroupedFiles => {
            let reserved = (parallelism / 3).max(1);
            let workers = parallelism.saturating_sub(reserved).max(1);
            (workers, workers)
        }
    };
    let queue = &WorkQueue::new(capacity, workers);
    let started = AtomicUsize::new(0);
    let index = AtomicUsize::new(0);
    thread::scope(|scope| {
        let _producer = QueueLifetime {
            queue,
            producer: true,
        };
        discover(&|group, path| {
            if started.load(Ordering::Relaxed) < workers
                && started.fetch_add(1, Ordering::Relaxed) < workers
            {
                spawn_scoped_worker(scope, || {
                    let _worker = QueueLifetime {
                        queue,
                        producer: false,
                    };
                    consume(&mut std::iter::from_fn(|| queue.pop()));
                });
            }
            let index = index.fetch_add(1, Ordering::Relaxed);
            queue.push(DiscoveredFile { group, index, path });
        });
    });
}

struct WorkQueue {
    capacity: usize,
    state: Mutex<QueueState>,
    readable: Condvar,
    writable: Condvar,
}

struct QueueState {
    pending: VecDeque<DiscoveredFile>,
    closed: bool,
    waiting_readers: usize,
    waiting_writers: usize,
}

impl WorkQueue {
    fn new(capacity: usize, workers: usize) -> Self {
        Self {
            capacity,
            state: Mutex::new(QueueState {
                pending: VecDeque::with_capacity(workers.min(capacity)),
                closed: false,
                waiting_readers: 0,
                waiting_writers: 0,
            }),
            readable: Condvar::new(),
            writable: Condvar::new(),
        }
    }

    fn push(&self, file: DiscoveredFile) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.pending.len() == self.capacity && !state.closed {
            state.waiting_writers += 1;
            state = self
                .writable
                .wait_while(state, |state| {
                    state.pending.len() == self.capacity && !state.closed
                })
                .unwrap_or_else(PoisonError::into_inner);
            state.waiting_writers -= 1;
        }
        if !state.closed {
            state.pending.push_back(file);
            let wake_reader = state.waiting_readers != 0;
            drop(state);
            if wake_reader {
                self.readable.notify_one();
            }
        }
    }

    fn pop(&self) -> Option<DiscoveredFile> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.pending.is_empty() && !state.closed {
            state.waiting_readers += 1;
            state = self
                .readable
                .wait_while(state, |state| state.pending.is_empty() && !state.closed)
                .unwrap_or_else(PoisonError::into_inner);
            state.waiting_readers -= 1;
        }
        let wake_producer = state.waiting_writers != 0;
        let file = state.pending.pop_front();
        drop(state);
        if wake_producer {
            self.writable.notify_one();
        }
        file
    }

    fn close(&self) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .closed = true;
        self.readable.notify_all();
        self.writable.notify_all();
    }
}

struct QueueLifetime<'a> {
    queue: &'a WorkQueue,
    producer: bool,
}

impl Drop for QueueLifetime<'_> {
    fn drop(&mut self) {
        if self.producer || thread::panicking() {
            self.queue.close();
        }
    }
}

#[cfg(test)]
mod tests;
