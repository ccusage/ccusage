use std::{
    env,
    path::{Path, PathBuf},
};

use ccusage_adapter_common::collect_files_matching;

const DSH_HOME_ENV: &str = "DSH_HOME";
const SESSIONS_DIR: &str = "sessions";

pub(super) fn discover_session_files() -> Vec<PathBuf> {
    let Some(home) = dsh_home() else {
        return Vec::new();
    };
    let sessions = home.join(SESSIONS_DIR);
    let mut files = Vec::new();
    collect_files_matching(
        &sessions,
        &|path| session_file_version(path).is_some(),
        &mut files,
    );
    files.sort();
    files.dedup();
    files
}

pub(super) fn session_file_version(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_str()?;
    let name = name.strip_suffix(".zstd").unwrap_or(name);
    let stem = name.strip_suffix(".jsonl")?;
    if stem == "session" {
        return Some(0);
    }
    let version = stem.strip_prefix("session.v")?.parse().ok()?;
    (version > 0).then_some(version)
}

fn dsh_home() -> Option<PathBuf> {
    if let Some(value) = env::var_os(DSH_HOME_ENV) {
        let value = value.to_string_lossy();
        let value = value.trim();
        if !value.is_empty() {
            return Some(PathBuf::from(value));
        }
    }
    crate::home::home_dir().map(|home| home.join(".dsh"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    #[test]
    fn discovers_supported_session_filename_generations_and_compression() {
        let fixture = fs_fixture!({
            "sessions/cwd/session-a/session.jsonl": "{}\n",
            "sessions/cwd/session-a/session.jsonl.zstd": "compressed bytes",
            "sessions/cwd/session-a/session.v3.jsonl": "{}\n",
            "sessions/cwd/session-a/session.v3.jsonl.zstd": "compressed bytes",
            "sessions/cwd/session-a/session.v4.jsonl": "{}\n",
            "sessions/cwd/session-a/session.v4.jsonl.zstd": "compressed bytes",
            "sessions/cwd/session-a/session.v2.jsonl.zstd": "compressed bytes",
            "sessions/cwd/session-a/other.jsonl": "{}\n",
            "sessions/cwd/session-a/session.vx.jsonl": "{}\n",
            "other/session.v4.jsonl": "{}\n",
        });
        let _cleanup = EnvVarGuard::set(DSH_HOME_ENV, fixture.root());

        let files = discover_session_files();

        assert_eq!(
            files,
            vec![
                fixture.path("sessions/cwd/session-a/session.jsonl"),
                fixture.path("sessions/cwd/session-a/session.jsonl.zstd"),
                fixture.path("sessions/cwd/session-a/session.v2.jsonl.zstd"),
                fixture.path("sessions/cwd/session-a/session.v3.jsonl"),
                fixture.path("sessions/cwd/session-a/session.v3.jsonl.zstd"),
                fixture.path("sessions/cwd/session-a/session.v4.jsonl"),
                fixture.path("sessions/cwd/session-a/session.v4.jsonl.zstd"),
            ]
        );
    }

    #[test]
    fn treats_blank_dsh_home_as_unset() {
        let _cleanup = EnvVarGuard::set(DSH_HOME_ENV, "  ");
        let home = dsh_home();
        assert_eq!(home, crate::home::home_dir().map(|home| home.join(".dsh")));
    }

    #[test]
    fn extracts_generation_from_plain_and_compressed_names() {
        assert_eq!(session_file_version(Path::new("session.jsonl")), Some(0));
        assert_eq!(session_file_version(Path::new("session.v3.jsonl")), Some(3));
        assert_eq!(
            session_file_version(Path::new("session.v4.jsonl.zstd")),
            Some(4)
        );
        assert_eq!(session_file_version(Path::new("session.v0.jsonl")), None);
        assert_eq!(session_file_version(Path::new("session.jsonl.zst")), None);
    }
}
