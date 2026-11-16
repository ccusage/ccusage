// Defined in ccusage-cli so the config crate can use it without depending on
// this crate; re-exported so existing callers keep one import path.
pub use ccusage_cli::home::expand_home_path;
