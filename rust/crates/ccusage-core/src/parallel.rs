use std::thread::{self, JoinHandle};

#[cfg(feature = "test-environment")]
use crate::cli::environment::Environment;

#[inline]
pub fn spawn_worker<F, T>(work: F) -> JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    #[cfg(feature = "test-environment")]
    let work = {
        let environment = Environment::current();
        move || environment.run(work)
    };
    thread::spawn(work)
}
