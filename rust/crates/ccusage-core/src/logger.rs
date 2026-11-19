use crate::cli::{SharedArgs, environment::Environment};

pub fn debug_log(shared: &SharedArgs, message: impl AsRef<str>) {
    if shared.debug {
        eprintln!("{}", message.as_ref());
    }
}

pub fn log_level() -> Option<u8> {
    Environment::current()
        .var("LOG_LEVEL")
        .ok()
        .and_then(|value| value.parse::<u8>().ok())
}
