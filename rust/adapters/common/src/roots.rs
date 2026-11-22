use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
};

use ccusage_core::cli::{environment::Environment, home};

#[derive(Clone, Copy)]
pub enum RootsEnv<'a> {
    List(&'a str),
    Single(&'a str),
}

impl RootsEnv<'_> {
    pub fn configured(self) -> Option<Vec<PathBuf>> {
        self.configured_in(&Environment::current())
    }

    fn configured_in(self, environment: &Environment) -> Option<Vec<PathBuf>> {
        let (name, separator) = match self {
            Self::List(name) => (name, Some(',')),
            Self::Single(name) => (name, None),
        };
        environment.var_os(name).and_then(|raw| {
            home::parse_paths(&raw, separator, home::home_dir_in(environment).as_deref())
        })
    }

    pub fn candidates(self, defaults: impl FnOnce() -> Vec<PathBuf>) -> Vec<PathBuf> {
        self.configured()
            .unwrap_or_else(|| default_candidates(false, defaults))
    }
}

pub fn default_candidates(debug: bool, defaults: impl FnOnce() -> Vec<PathBuf>) -> Vec<PathBuf> {
    let paths = defaults();
    if paths.is_empty()
        && home::home_dir().is_none()
        && (debug || ccusage_core::log_level().is_some_and(|level| level >= 4))
    {
        static NO_HOME: std::sync::Once = std::sync::Once::new();
        NO_HOME.call_once(|| {
            ccusage_core::errln!("Unable to resolve default data roots: home directory is not set")
        });
    }
    paths
}

pub fn resolve(env: RootsEnv<'_>, defaults: impl FnOnce() -> Vec<PathBuf>) -> Vec<PathBuf> {
    directories(env.candidates(defaults))
}

pub fn from_list(raw: &str) -> Option<Vec<PathBuf>> {
    home::parse_paths(raw.as_ref(), Some(','), home::home_dir().as_deref()).map(directories)
}

pub fn directories(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    directories_with(paths, |_, _| {})
}

pub fn directories_with(
    paths: impl IntoIterator<Item = PathBuf>,
    mut ignored: impl FnMut(&Path, io::Error),
) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|path| {
            let identity = fs::metadata(path).and_then(|metadata| {
                if metadata.is_dir() {
                    Ok(home::path_identity(path))
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::NotADirectory,
                        "not a directory",
                    ))
                }
            });
            match identity {
                Ok(identity) => seen.insert(identity),
                Err(error) => {
                    ignored(path, error);
                    false
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::fs_fixture;

    #[test]
    fn list_roots_trim_expand_and_deduplicate_without_reordering() {
        let fixture = fs_fixture!({});
        let first = fixture.create_dir_all("first");
        let second = fixture.create_dir_all("second");
        Environment::with_vars([
            ("HOME", Some(fixture.root().into())),
            (
                "CCUSAGE_TEST_ROOTS",
                Some(" ~/first, ~/second, ~/first/., , ~/missing ".into()),
            ),
        ])
        .run(|| {
            assert_eq!(
                resolve(RootsEnv::List("CCUSAGE_TEST_ROOTS"), Vec::new),
                vec![first, second]
            );
        })
    }

    #[test]
    fn blank_lists_use_defaults_but_missing_configured_roots_do_not() {
        let fixture = fs_fixture!({});
        let fallback = fixture.create_dir_all("fallback");
        for value in ["", " ", " , , "] {
            Environment::with_var("CCUSAGE_TEST_ROOTS", value).run(|| {
                assert_eq!(
                    resolve(RootsEnv::List("CCUSAGE_TEST_ROOTS"), || vec![
                        fallback.clone()
                    ]),
                    vec![fallback.clone()]
                );
            })
        }
        Environment::with_var("CCUSAGE_TEST_ROOTS", fixture.path("missing")).run(|| {
            assert!(resolve(RootsEnv::List("CCUSAGE_TEST_ROOTS"), || vec![fallback]).is_empty());
        })
    }

    #[test]
    fn single_roots_keep_commas_in_the_directory_name() {
        let fixture = fs_fixture!({});
        let root = fixture.create_dir_all("first,second");
        Environment::with_var("CCUSAGE_TEST_ROOTS", format!(" {} ", root.display())).run(|| {
            assert_eq!(
                resolve(RootsEnv::Single("CCUSAGE_TEST_ROOTS"), Vec::new),
                vec![root]
            );
        })
    }

    #[test]
    fn invalid_roots_are_reported_without_dropping_healthy_roots() {
        let fixture = fs_fixture!({});
        let healthy = fixture.create_dir_all("healthy");
        let missing = fixture.path("missing");
        let file = fixture.write_file("file", "");
        let mut ignored = Vec::new();

        let resolved = directories_with(
            [missing.clone(), file.clone(), healthy.clone()],
            |path, error| ignored.push((path.to_path_buf(), error.kind())),
        );

        assert_eq!(resolved, vec![healthy]);
        assert_eq!(
            ignored,
            [
                (missing, io::ErrorKind::NotFound),
                (file, io::ErrorKind::NotADirectory),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_link_roots_are_read_once_using_the_first_path() {
        let fixture = fs_fixture!({});
        let root = fixture.create_dir_all("root");
        let alias = fixture.path("alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        Environment::with_var(
            "CCUSAGE_TEST_ROOTS",
            format!("{},{}", alias.display(), root.display()),
        )
        .run(|| {
            assert_eq!(
                resolve(RootsEnv::List("CCUSAGE_TEST_ROOTS"), Vec::new),
                vec![alias]
            );
        })
    }

    #[test]
    fn tilde_without_a_home_does_not_use_a_literal_directory_or_defaults() {
        let fixture = fs_fixture!({});
        let fallback = fixture.create_dir_all("fallback");
        Environment::with_vars([
            ("HOME", None),
            ("USERPROFILE", None),
            ("HOMEDRIVE", None),
            ("HOMEPATH", None),
            ("CCUSAGE_TEST_ROOTS", Some("~/store".into())),
        ])
        .run(|| {
            assert!(resolve(RootsEnv::List("CCUSAGE_TEST_ROOTS"), || vec![fallback]).is_empty());
        })
    }
}
