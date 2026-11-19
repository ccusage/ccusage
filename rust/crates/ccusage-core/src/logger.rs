use crate::cli::{SharedArgs, environment::Environment};

pub fn debug_log(shared: &SharedArgs, message: impl AsRef<str>) {
    if shared.debug {
        ccusage_terminal::errln!("{}", message.as_ref());
    }
}

pub fn log_level() -> Option<u8> {
    Environment::current()
        .var_os("LOG_LEVEL")
        .and_then(|value| value.into_string().ok())
        .and_then(|value| value.parse::<u8>().ok())
}
