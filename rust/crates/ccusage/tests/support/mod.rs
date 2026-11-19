use std::{env, fs, path::Path, process::Command};

use ccusage_cli::environment::Environment;

pub(crate) fn hermetic_command(root: &Path) -> Command {
    hermetic_command_with_tools(root, &[])
}

pub(crate) fn hermetic_command_with_tools(root: &Path, tools: &[&str]) -> Command {
    let dir = |name: &str| {
        let path = root.join(name);
        fs::create_dir_all(&path).expect("failed to create hermetic directory");
        path
    };

    let environment = Environment::current();
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
        .envs(
            environment
                .var_os("TZDIR")
                .map(|zoneinfo| ("TZDIR", zoneinfo)),
        )
        .env("NO_COLOR", "1");
    if !tools.is_empty() {
        let path = environment
            .var_os("PATH")
            .expect("external test tools require a PATH");
        let directories = tools.iter().map(|tool| {
            let executable = format!("{tool}{}", env::consts::EXE_SUFFIX);
            env::split_paths(&path)
                .filter(|dir| dir.is_absolute())
                .find(|dir| dir.join(&executable).is_file())
                .unwrap_or_else(|| {
                    panic!("required external test tool {tool} is missing from PATH")
                })
        });
        command.env(
            "PATH",
            env::join_paths(directories).expect("invalid test tool path"),
        );
    }
    command
}
