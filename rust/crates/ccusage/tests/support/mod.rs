//! The one way an integration test runs the `ccusage` binary.

use std::{fs, path::Path, process::Command};

/// Builds a `ccusage` command that can see nothing outside `root`: a cleared
/// environment, every home directory inside `root`, and a working directory
/// inside `root` because config discovery reads `./.ccusage`.
///
/// An agent store variable left unset resolves under the empty fixture home,
/// so a test sets only the stores it seeds. The clock is the real one unless
/// the test sets `CCUSAGE_TEST_NOW`, and `COLUMNS` and `LOG_LEVEL` keep their
/// production defaults unless the test sets them.
pub fn hermetic_command(root: &Path) -> Command {
    let dir = |name: &str| {
        let path = root.join(name);
        fs::create_dir_all(&path).expect("failed to create hermetic directory");
        path
    };

    let mut command = Command::new(env!("CARGO_BIN_EXE_ccusage"));
    command
        .env_clear()
        .current_dir(dir("cwd"))
        .env("HOME", dir("home"))
        .env("USERPROFILE", dir("home"))
        .env("XDG_CONFIG_HOME", dir("xdg-config"))
        .env("XDG_DATA_HOME", dir("xdg-data"))
        .env("XDG_CACHE_HOME", dir("xdg-cache"))
        .env("TMPDIR", dir("tmp"))
        .env("TZ", "UTC")
        // jiff reads named zones such as Asia/Tokyo from the zoneinfo database,
        // which the Nix test sandbox only provides through TZDIR.
        .envs(std::env::var_os("TZDIR").map(|zoneinfo| ("TZDIR", zoneinfo)))
        .env("NO_COLOR", "1");
    command
}
