use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Condvar, Mutex, PoisonError},
    thread,
};

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
        &mut |emit| discover(&mut |path| emit(0, path)),
        &|file| {
            let result = read(&file.path);
            (file.path, result)
        },
    )
}

/// Reads files as discovery yields them. Parallel results have no ordering guarantee.
pub fn read_discovered_files<T: Send>(
    single_thread: bool,
    discover: &mut DiscoverFiles<'_>,
    read: &(dyn Fn(&DiscoveredFile) -> T + Sync),
) -> Vec<(DiscoveredFile, T)> {
    collect_discovered(single_thread, discover, &|file| {
        let result = read(&file);
        (file, result)
    })
}

fn collect_discovered<T: Send>(
    single_thread: bool,
    discover: &mut DiscoverFiles<'_>,
    read: &(dyn Fn(DiscoveredFile) -> T + Sync),
) -> Vec<T> {
    if single_thread {
        let mut results = Vec::new();
        index_discovered(discover, &mut |file| results.push(read(file)));
        return results;
    }

    let batches = Mutex::new(Vec::new());
    let consume = |files: &mut dyn Iterator<Item = DiscoveredFile>| {
        let results = files.map(read).collect::<Vec<_>>();
        batches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(results);
    };
    read_discovered_in_parallel(discover, &consume);
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
fn read_discovered_in_parallel(discover: &mut DiscoverFiles<'_>, consume: &ConsumeFiles<'_>) {
    let parallelism = thread::available_parallelism().map_or(1, usize::from);
    let workers = parallelism.saturating_sub(1).max(1);
    let queue = &WorkQueue::new(workers);
    thread::scope(|scope| {
        let _producer = QueueLifetime {
            queue,
            producer: true,
        };
        let mut started = 0;
        index_discovered(discover, &mut |file| {
            if started < workers {
                scope.spawn(|| {
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

struct WorkQueue {
    capacity: usize,
    refill_at: usize,
    state: Mutex<QueueState>,
    readable: Condvar,
    writable: Condvar,
}

struct QueueState {
    pending: VecDeque<DiscoveredFile>,
    closed: bool,
    waiting_readers: usize,
    waiting_writer: bool,
}

impl WorkQueue {
    fn new(workers: usize) -> Self {
        let capacity = workers.saturating_mul(4);
        Self {
            capacity,
            refill_at: capacity / 2,
            state: Mutex::new(QueueState {
                pending: VecDeque::with_capacity(capacity),
                closed: false,
                waiting_readers: 0,
                waiting_writer: false,
            }),
            readable: Condvar::new(),
            writable: Condvar::new(),
        }
    }

    fn push(&self, file: DiscoveredFile) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.pending.len() == self.capacity {
            state.waiting_writer = true;
            state = self
                .writable
                .wait_while(state, |state| {
                    state.pending.len() > self.refill_at && !state.closed
                })
                .unwrap_or_else(PoisonError::into_inner);
            state.waiting_writer = false;
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
        let wake_producer = state.waiting_writer && state.pending.len() == self.refill_at + 1;
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
