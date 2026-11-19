use std::thread::{self, JoinHandle, Scope, ScopedJoinHandle};

#[cfg(feature = "test-environment")]
use crate::cli::environment::Environment;

#[inline]
pub fn spawn_worker<F, T>(work: F) -> JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    #[cfg(feature = "test-environment")]
    let work = with_environment(work);
    thread::spawn(work)
}

#[inline]
pub fn spawn_scoped_worker<'scope, F, T>(
    scope: &'scope Scope<'scope, '_>,
    work: F,
) -> ScopedJoinHandle<'scope, T>
where
    F: FnOnce() -> T + Send + 'scope,
    T: Send + 'scope,
{
    #[cfg(feature = "test-environment")]
    let work = with_environment(work);
    scope.spawn(work)
}

#[cfg(feature = "test-environment")]
fn with_environment<F, T>(work: F) -> impl FnOnce() -> T
where
    F: FnOnce() -> T,
{
    let environment = Environment::current();
    move || environment.run(work)
}

#[cfg(all(test, feature = "test-environment"))]
mod tests {
    use super::*;

    #[test]
    fn scoped_worker_inherits_fixture_environment() {
        let key = "CCUSAGE_SCOPED_WORKER_TEST";
        let value = Environment::with_var(key, "fixture").run(|| {
            thread::scope(|scope| {
                spawn_scoped_worker(scope, || Environment::current().var_os(key))
                    .join()
                    .unwrap()
            })
        });
        assert_eq!(value.as_deref(), Some(std::ffi::OsStr::new("fixture")));
    }
}
