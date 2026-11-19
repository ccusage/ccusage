use std::thread::{self, JoinHandle};

use crate::cli::environment::Environment;

#[inline]
pub fn spawn_worker<F, T>(work: F) -> JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let environment = Environment::current();
    thread::spawn(move || environment.run(work))
}
