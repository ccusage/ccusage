use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Condvar, Mutex, PoisonError},
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
        index_discovered(discover, &mut |file| results.push(read(file)));
        return results;
    }

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

fn index_discovered(discover: &mut DiscoverFiles<'_>, emit: &mut dyn FnMut(DiscoveredFile)) {
    let mut index = 0;
    discover(&mut |group, path| {
        emit(DiscoveredFile { group, index, path });
        index += 1;
    });
}

type ConsumeFiles<'a> = dyn Fn(&mut dyn Iterator<Item = DiscoveredFile>) + Sync + 'a;

#[inline(never)]
fn read_discovered_in_parallel(
    workload: ReadWorkload,
    discover: &mut DiscoverFiles<'_>,
    consume: &ConsumeFiles<'_>,
) {
    let parallelism = thread::available_parallelism().map_or(1, usize::from);
    let (workers, capacity) = match workload {
        ReadWorkload::PlainPaths => (parallelism, PLAIN_PATH_BACKLOG),
        ReadWorkload::GroupedFiles => {
            let workers = grouped_workers(parallelism);
            (workers, workers)
        }
    };
    let queue = &WorkQueue::new(capacity, workers);
    thread::scope(|scope| {
        let _producer = QueueLifetime {
            queue,
            producer: true,
        };
        let mut started = 0;
        index_discovered(discover, &mut |file| {
            if started < workers {
                spawn_scoped_worker(scope, || {
                    let _worker = QueueLifetime {
                        queue,
                        producer: false,
                    };
                    consume(&mut std::iter::from_fn(|| queue.pop()));
                });
                started += 1;
            }
            queue.push(file);
        });
    });
}

const GROUPED_EVERY_CORE_UP_TO: usize = 16;

// NOTE(2026-11-21, #2476): Codex parsing gets faster with every core on 2-, 4- and 12-core machines, but on a 32-vCPU box it stops at about 22 parsers, and 32 parsers only add 23-29% CPU. Above 16 cores use 16 parsers, or two thirds of the cores once that is more (25 cores and up), until the contention past that point is understood.
fn grouped_workers(parallelism: usize) -> usize {
    parallelism
        .min(GROUPED_EVERY_CORE_UP_TO)
        .max(parallelism - parallelism / 3)
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
}

impl WorkQueue {
    fn new(capacity: usize, workers: usize) -> Self {
        Self {
            capacity,
            state: Mutex::new(QueueState {
                pending: VecDeque::with_capacity(workers.min(capacity)),
                closed: false,
                waiting_readers: 0,
            }),
            readable: Condvar::new(),
            writable: Condvar::new(),
        }
    }

    fn push(&self, file: DiscoveredFile) {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut state = self
            .writable
            .wait_while(state, |state| {
                state.pending.len() == self.capacity && !state.closed
            })
            .unwrap_or_else(PoisonError::into_inner);
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
        let wake_producer = state.pending.len() == self.capacity;
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
