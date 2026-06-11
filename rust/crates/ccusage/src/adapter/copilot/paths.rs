use std::{env, path::Path, path::PathBuf};

use crate::Result;

pub(crate) const COPILOT_CONFIG_DIR_ENV: &str = "COPILOT_CONFIG_DIR";

/// Legacy OTel env vars that now only trigger a warning.
pub(crate) const LEGACY_OTEL_ENV_VARS: &[&str] = &[
    "COPILOT_OTEL_FILE_EXPORTER_PATH",
    "COPILOT_OTEL_DEDUP",
    "COPILOT_PREFER_OTEL",
];

const COPILOT_DIR_NAME: &str = ".copilot";
const SESSION_STATE_DIR_NAME: &str = "session-state";
const EVENTS_FILENAME: &str = "events.jsonl";

pub(super) fn session_state_paths() -> Result<Vec<PathBuf>> {
    let Some(base) = copilot_base_dir() else {
        return Ok(Vec::new());
    };
    let session_state_dir = base.join(SESSION_STATE_DIR_NAME);
    if !session_state_dir.is_dir() {
        return Ok(Vec::new());
    }
    Ok(session_state_event_files(&session_state_dir))
}

pub(super) fn has_any_session_state_event_file() -> bool {
    let Some(base) = copilot_base_dir() else {
        return false;
    };
    let session_state_dir = base.join(SESSION_STATE_DIR_NAME);
    let Ok(entries) = std::fs::read_dir(&session_state_dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        path.is_dir() && path.join(EVENTS_FILENAME).is_file()
    })
}

fn copilot_base_dir() -> Option<PathBuf> {
    if let Some(value) = env::var_os(COPILOT_CONFIG_DIR_ENV) {
        let trimmed_path = PathBuf::from(trim_os_string(&value));
        if !trimmed_path.as_os_str().is_empty() {
            return if trimmed_path.is_dir() {
                Some(trimmed_path)
            } else {
                None
            };
        }
    }
    crate::home::home_dir().map(|home| home.join(COPILOT_DIR_NAME))
}

fn trim_os_string(value: &std::ffi::OsStr) -> std::ffi::OsString {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let bytes = value.as_bytes();
        if let Ok(s) = std::str::from_utf8(bytes) {
            return std::ffi::OsString::from(s.trim());
        }
        let start = bytes
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .unwrap_or(bytes.len());
        let end = bytes
            .iter()
            .rposition(|b| !b.is_ascii_whitespace())
            .map_or(start, |i| i + 1);
        std::ffi::OsString::from_vec(bytes[start..end].to_vec())
    }
    #[cfg(not(unix))]
    {
        std::ffi::OsString::from(value.to_string_lossy().trim())
    }
}

fn session_state_event_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let events = path.join(EVENTS_FILENAME);
        if events.is_file() {
            files.push(events);
        }
    }
    files.sort();
    files
}

pub(crate) fn legacy_otel_env_vars_in_use() -> Vec<&'static str> {
    LEGACY_OTEL_ENV_VARS
        .iter()
        .copied()
        .filter(|name| {
            env::var_os(name)
                .map(|value| !trim_os_string(&value).is_empty())
                .unwrap_or(false)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    use super::*;

    fn env_scope(overrides: &[(&'static str, Option<&str>)]) -> EnvVarGuard {
        let overrides = overrides
            .iter()
            .map(|(key, value)| (*key, value.map(OsStr::new)))
            .collect::<Vec<_>>();
        EnvVarGuard::set_many(&overrides)
    }

    #[test]
    fn enumerates_session_state_events_files() {
        let fixture = fs_fixture!({
            "session-state/aaaa-1111/events.jsonl": "",
            "session-state/aaaa-1111/workspace.yaml": "cwd: /tmp\n",
            "session-state/bbbb-2222/events.jsonl": "",
            "session-state/cccc-3333/events.jsonl": "",
            "session-state/dddd-empty/.keep": "",
        });
        let _env = env_scope(&[(
            COPILOT_CONFIG_DIR_ENV,
            Some(fixture.root().to_str().unwrap()),
        )]);

        let paths = session_state_paths().unwrap();
        assert_eq!(
            paths.len(),
            3,
            "expected three events.jsonl entries, got {paths:?}"
        );
        for uuid in ["aaaa-1111", "bbbb-2222", "cccc-3333"] {
            let expected = fixture.path(format!("session-state/{uuid}/events.jsonl"));
            assert!(
                paths.contains(&expected),
                "missing entry for {uuid} in {paths:?}"
            );
        }
    }

    #[test]
    fn respects_copilot_config_dir_env() {
        let fixture = fs_fixture!({
            "session-state/alpha/events.jsonl": "",
            // Files under `otel/` exist on disk but must NOT be discovered —
            // the OTel source is intentionally ignored after this change.
            "otel/trace.jsonl": "",
        });
        let _env = env_scope(&[(
            COPILOT_CONFIG_DIR_ENV,
            Some(fixture.root().to_str().unwrap()),
        )]);

        let paths = session_state_paths().unwrap();
        assert_eq!(
            paths.len(),
            1,
            "expected one session-state events.jsonl, got {paths:?}"
        );
        assert!(
            paths[0].ends_with("session-state/alpha/events.jsonl"),
            "got {paths:?}"
        );
    }

    #[test]
    fn ignores_otel_directory_even_when_present() {
        let fixture = fs_fixture!({
            "otel/trace.jsonl": "{\"type\":\"span\"}",
            "otel/nested/another.jsonl": "{\"type\":\"span\"}",
        });
        let _env = env_scope(&[(
            COPILOT_CONFIG_DIR_ENV,
            Some(fixture.root().to_str().unwrap()),
        )]);

        // No session-state dir, only an `otel/` dir with files. Discovery
        // must return zero paths — OTel is no longer a data source.
        let paths = session_state_paths().unwrap();
        assert!(
            paths.is_empty(),
            "OTel files must not be discovered, got {paths:?}"
        );
    }

    #[test]
    fn copilot_otel_file_exporter_path_env_is_ignored() {
        let fixture = fs_fixture!({
            "explicit-otel.jsonl": "{\"type\":\"span\"}",
        });
        let _env = env_scope(&[
            (
                "COPILOT_OTEL_FILE_EXPORTER_PATH",
                Some(fixture.path("explicit-otel.jsonl").to_str().unwrap()),
            ),
            // No session-state dir; the only thing pointed at is the
            // legacy OTel exporter env var, which must be ignored.
            (
                COPILOT_CONFIG_DIR_ENV,
                Some(fixture.root().to_str().unwrap()),
            ),
        ]);

        let paths = session_state_paths().unwrap();
        assert!(
            paths.is_empty(),
            "COPILOT_OTEL_FILE_EXPORTER_PATH must be inert, got {paths:?}"
        );
    }

    #[test]
    fn missing_directories_yield_no_sources() {
        let fixture = fs_fixture!({
            ".keep": "",
        });
        let _env = env_scope(&[(
            COPILOT_CONFIG_DIR_ENV,
            Some(fixture.root().to_str().unwrap()),
        )]);

        let paths = session_state_paths().unwrap();
        assert!(paths.is_empty(), "expected zero paths, got {paths:?}");
    }

    #[test]
    fn nonexistent_copilot_config_dir_does_not_fall_back_to_default() {
        // Regression: an explicit `COPILOT_CONFIG_DIR` pointing at a
        // directory that doesn't exist must NOT silently fall through to
        // `~/.copilot`. The user explicitly chose a different location,
        // and reading from the default install would surprise them with
        // data they didn't ask for. We instead return an empty Vec so
        // `ccusage copilot ...` prints "No usage data found".
        //
        // Hermetic: override HOME to a temp dir that DOES contain a
        // `.copilot/session-state/<uuid>/events.jsonl` fixture. Under a
        // buggy implementation that silently fell through to
        // `~/.copilot`, the fall-through would find the fixture and the
        // assertion would fail; under the correct implementation the
        // explicit override wins and the result is empty regardless of
        // HOME contents.
        let home_fixture = fs_fixture!({
            ".copilot/session-state/sentinel/events.jsonl":
                "{\"sentinel\":\"would only be seen if the override silently fell through\"}\n",
        });
        let _env = env_scope(&[
            ("HOME", Some(home_fixture.root().to_str().unwrap())),
            // Clear Windows alternatives so home_dir() can't pick them up.
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
            (
                COPILOT_CONFIG_DIR_ENV,
                Some("/definitely/does/not/exist/on/any/test/host"),
            ),
        ]);

        let paths = session_state_paths().unwrap();
        assert!(
            paths.is_empty(),
            "explicit override at a nonexistent path must yield zero \
             sources, not silently fall back to ~/.copilot; got {paths:?}"
        );
    }

    #[test]
    fn legacy_otel_env_vars_in_use_reports_only_set_non_empty_vars() {
        // All unset → empty.
        let _env = env_scope(&[
            ("COPILOT_OTEL_FILE_EXPORTER_PATH", None),
            ("COPILOT_OTEL_DEDUP", None),
            ("COPILOT_PREFER_OTEL", None),
        ]);
        assert!(legacy_otel_env_vars_in_use().is_empty());
        drop(_env);

        // One set → reported.
        let _env = env_scope(&[
            ("COPILOT_OTEL_FILE_EXPORTER_PATH", Some("/tmp/x.jsonl")),
            ("COPILOT_OTEL_DEDUP", None),
            ("COPILOT_PREFER_OTEL", None),
        ]);
        assert_eq!(
            legacy_otel_env_vars_in_use(),
            vec!["COPILOT_OTEL_FILE_EXPORTER_PATH"]
        );
        drop(_env);

        // All three set → all reported in declaration order.
        let _env = env_scope(&[
            ("COPILOT_OTEL_FILE_EXPORTER_PATH", Some("/tmp/x.jsonl")),
            ("COPILOT_OTEL_DEDUP", Some("strict")),
            ("COPILOT_PREFER_OTEL", Some("1")),
        ]);
        assert_eq!(
            legacy_otel_env_vars_in_use(),
            vec![
                "COPILOT_OTEL_FILE_EXPORTER_PATH",
                "COPILOT_OTEL_DEDUP",
                "COPILOT_PREFER_OTEL",
            ]
        );
        drop(_env);

        // Whitespace-only value → treated as unset (matches the
        // COPILOT_CONFIG_DIR convention in `copilot_base_dir`).
        let _env = env_scope(&[
            ("COPILOT_OTEL_FILE_EXPORTER_PATH", Some("   ")),
            ("COPILOT_OTEL_DEDUP", None),
            ("COPILOT_PREFER_OTEL", None),
        ]);
        assert!(
            legacy_otel_env_vars_in_use().is_empty(),
            "whitespace-only value should not count as set"
        );
    }

    #[test]
    #[cfg(unix)]
    fn copilot_config_dir_with_non_utf8_path_is_authoritative_not_silently_dropped() {
        // Regression: a non-UTF-8 directory path (legal on Unix) used to
        // fall through the `env::var(...).ok()` lossy String conversion
        // (`Err(NotUnicode)` → `None`), silently activating the
        // `~/.copilot` default — directly contradicting the
        // "override is authoritative" invariant documented on
        // `copilot_base_dir`. Now reads via `env::var_os` so non-UTF-8
        // values flow through unchanged.
        //
        // Hermetic: HOME is overridden to a temp dir that DOES contain
        // a `.copilot/session-state/<uuid>/events.jsonl` fixture. Under
        // the buggy code, the lossy `Err(NotUnicode) → None` path would
        // fall through to `~/.copilot` (= temp dir here), find the
        // fixture, and return a non-empty Vec — failing the assertion.
        // Under the fixed code the non-UTF-8 override is honored and
        // points at a nonexistent directory, so the result is empty.
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let home_fixture = fs_fixture!({
            ".copilot/session-state/sentinel/events.jsonl":
                "{\"sentinel\":\"would only be seen if the override silently fell through\"}\n",
        });
        let _env = env_scope(&[
            ("HOME", Some(home_fixture.root().to_str().unwrap())),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
            // Clear here so the guard captures the prior value and restores it
            // after the direct non-UTF-8 `set_var` below.
            (COPILOT_CONFIG_DIR_ENV, None),
        ]);
        let mut non_utf8 = b"/tmp/ccusage-test-".to_vec();
        non_utf8.extend_from_slice(b"\xff\xfe-non-utf8-dir-does-not-exist");
        let override_value = OsString::from_vec(non_utf8);
        unsafe { env::set_var(COPILOT_CONFIG_DIR_ENV, &override_value) };

        let paths = session_state_paths().unwrap();
        assert!(
            paths.is_empty(),
            "non-UTF-8 override at a nonexistent path must be honored as an \
             explicit override (yielding zero sources), not coerced to \
             'env unset' and silently fall back to ~/.copilot; got {paths:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn legacy_otel_env_var_with_non_utf8_value_is_still_reported() {
        // Regression mirror of the `env::var` → `env::var_os` fix in
        // `copilot_base_dir`: if a user (or test harness) sets one of
        // the deprecated OTel env vars to a non-UTF-8 byte sequence,
        // the deprecation warning MUST still fire. The previous
        // `env::var(name).ok()` flow would silently coerce
        // `Err(NotUnicode)` to `None` and report the var as unset.
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let _env = env_scope(&[
            ("COPILOT_OTEL_FILE_EXPORTER_PATH", None),
            ("COPILOT_OTEL_DEDUP", None),
            ("COPILOT_PREFER_OTEL", None),
        ]);
        let mut non_utf8 = b"/tmp/otel-".to_vec();
        non_utf8.extend_from_slice(b"\xff\xfe-non-utf8.jsonl");
        unsafe {
            env::set_var(
                "COPILOT_OTEL_FILE_EXPORTER_PATH",
                OsString::from_vec(non_utf8),
            )
        };

        assert_eq!(
            legacy_otel_env_vars_in_use(),
            vec!["COPILOT_OTEL_FILE_EXPORTER_PATH"],
            "non-UTF-8 value must still be detected as set (deprecation \
             warning would otherwise silently skip it)",
        );
    }
}
