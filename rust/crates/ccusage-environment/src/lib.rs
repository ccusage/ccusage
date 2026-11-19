use std::{env, ffi::OsString};

#[cfg(feature = "test-environment")]
use std::{cell::RefCell, collections::HashMap, ffi::OsStr, sync::Arc};

#[cfg(feature = "test-environment")]
type Overrides = Arc<HashMap<&'static str, Option<OsString>>>;

#[cfg(feature = "test-environment")]
thread_local! {
    static TEST_ENVIRONMENT: RefCell<Option<Overrides>> = const { RefCell::new(None) };
}

pub struct Environment {
    #[cfg(feature = "test-environment")]
    overrides: Option<Overrides>,
}

impl Environment {
    #[inline]
    pub fn current() -> Self {
        Self {
            #[cfg(feature = "test-environment")]
            overrides: TEST_ENVIRONMENT.with(|environment| environment.borrow().clone()),
        }
    }

    #[inline]
    pub fn var(&self, name: &str) -> Result<String, env::VarError> {
        match self.var_os(name) {
            Some(value) => value.into_string().map_err(env::VarError::NotUnicode),
            None => Err(env::VarError::NotPresent),
        }
    }

    #[inline]
    pub fn var_os(&self, name: &str) -> Option<OsString> {
        #[cfg(feature = "test-environment")]
        if let Some(value) = self.overrides.as_ref().and_then(|vars| vars.get(name)) {
            return value.clone();
        }
        env::var_os(name)
    }

    #[cfg(feature = "test-environment")]
    pub fn with_var(name: &'static str, value: impl AsRef<OsStr>) -> Self {
        Self::with_vars([(name, Some(value.as_ref().to_os_string()))])
    }

    #[cfg(feature = "test-environment")]
    pub fn with_vars(vars: impl IntoIterator<Item = (&'static str, Option<OsString>)>) -> Self {
        Self {
            overrides: Some(Arc::new(vars.into_iter().collect())),
        }
    }

    pub fn run<T>(&self, run: impl FnOnce() -> T) -> T {
        #[cfg(feature = "test-environment")]
        let _restore = TEST_ENVIRONMENT
            .with(|environment| RestoreEnvironment(environment.replace(self.overrides.clone())));
        run()
    }
}

#[cfg(feature = "test-environment")]
struct RestoreEnvironment(Option<Overrides>);

#[cfg(feature = "test-environment")]
impl Drop for RestoreEnvironment {
    fn drop(&mut self) {
        TEST_ENVIRONMENT.with(|environment| {
            environment.replace(self.0.take());
        });
    }
}
