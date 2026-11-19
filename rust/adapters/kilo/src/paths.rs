use std::path::{Path, PathBuf};

use ccusage_adapter_common::roots::{self, RootsEnv};

use ccusage_core::{Diagnostics, Skip, SkipKind};

pub(super) const KILO_DATA_DIR_ENV: &str = "KILO_DATA_DIR";
pub(super) const KILO_DB_FILE_NAME: &str = "kilo.db";

pub(super) fn paths(diagnostics: &Diagnostics, debug: bool) -> Vec<PathBuf> {
    if let Some(configured) = RootsEnv::List(KILO_DATA_DIR_ENV).configured() {
        return roots::directories_with(configured, |path, error| {
            diagnostics.record(Skip::unit(
                path,
                SkipKind::Unreadable,
                format!("Ignoring configured {KILO_DATA_DIR_ENV} root: {error}"),
            ));
        });
    }

    roots::directories(roots::default_candidates(debug, || {
        crate::cli::home::home_dir()
            .map(|home| home.join(".local/share/kilo"))
            .into_iter()
            .collect()
    }))
}

pub(super) fn db_path(kilo_dir: &Path) -> Option<PathBuf> {
    let path = kilo_dir.join(KILO_DB_FILE_NAME);
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use ccusage_test_support::{Environment, fs_fixture};

    use super::*;

    #[test]
    fn blank_data_dir_uses_the_home_default() {
        let fixture = fs_fixture!({});
        let default = fixture.create_dir_all(".local/share/kilo");
        let env = Environment::with_vars([
            ("HOME", Some(fixture.root().into())),
            (KILO_DATA_DIR_ENV, Some(" , ".into())),
        ]);
        let diagnostics = Diagnostics::new("kilo");

        assert_eq!(env.run(|| paths(&diagnostics, false)), [default]);
        assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 0);
    }

    #[test]
    fn configured_root_expands_home() {
        let fixture = fs_fixture!({});
        let custom = fixture.create_dir_all("custom");
        let env = Environment::with_vars([
            ("HOME", Some(fixture.root().into())),
            (KILO_DATA_DIR_ENV, Some("~/custom".into())),
        ]);

        assert_eq!(
            env.run(|| paths(&Diagnostics::new("kilo"), false)),
            [custom]
        );
    }

    #[test]
    fn invalid_configured_roots_are_reported_without_falling_back() {
        let fixture = fs_fixture!({ "file": "" });
        fixture.create_dir_all(".local/share/kilo");
        let env = Environment::with_vars([
            ("HOME", Some(fixture.root().into())),
            (KILO_DATA_DIR_ENV, Some("~/missing,~/file".into())),
        ]);
        let diagnostics = Diagnostics::new("kilo");

        assert!(env.run(|| paths(&diagnostics, false)).is_empty());
        assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 2);
    }
}
