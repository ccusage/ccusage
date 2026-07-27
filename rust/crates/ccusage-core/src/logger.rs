use std::env;

use crate::cli::SharedArgs;

pub fn debug_log(shared: &SharedArgs, message: impl AsRef<str>) {
    if shared.debug {
        eprintln!("{}", message.as_ref());
    }
}

pub fn log_level() -> Option<u8> {
    env::var("LOG_LEVEL")
        .ok()
        .and_then(|value| value.parse::<u8>().ok())
}
