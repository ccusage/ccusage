//! Database discovery for the Claude Science adapter.
//!
//! Claude Science stores conversation metadata (including token usage) in a
//! SQLite database whose location varies by platform. Users can point the
//! adapter at an explicit database file with `CLAUDE_SCIENCE_DB`; otherwise
//! the adapter scans well-known roots for a database that contains the
//! `frames` table.

use std::{env, fs, path::PathBuf};

use crate::Result;

pub(crate) const CLAUDE_SCIENCE_DB_ENV: &str = "CLAUDE_SCIENCE_DB";

/// Roots (relative to the user's home directory) that may hold the Claude
/// Science metadata database.
const DEFAULT_CLAUDE_SCIENCE_ROOTS: [&str; 6] = [
    ".claude-science",
    ".config/claude-science",
    ".config/Claude Science",
    ".local/share/claude-science",
    ".local/share/Claude Science",
    "Library/Application Support/Claude Science",
];

/// Known database filename used by the Claude Science daemon.
const DATABASE_FILE_NAME: &str = "operon-cli.db";

fn org_database_paths() -> Vec<PathBuf> {
    let Some(home) = crate::home::home_dir() else {
        return Vec::new();
    };
    let orgs = home.join(".claude-science/cs-switch-proxy/orgs");
    let Ok(entries) = fs::read_dir(&orgs) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for entry in entries.filter_map(std::result::Result::ok) {
        if !entry.file_type().is_ok_and(|type_| type_.is_dir()) {
            continue;
        }
        let path = entry.path().join(DATABASE_FILE_NAME);
        if path.is_file() {
            paths.push(path);
        }
    }
    paths
}

fn candidate_roots() -> Vec<PathBuf> {
    if let Ok(value) = env::var(CLAUDE_SCIENCE_DB_ENV) {
        return value
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .collect();
    }

    crate::home::home_dir()
        .into_iter()
        .flat_map(|home| {
            DEFAULT_CLAUDE_SCIENCE_ROOTS
                .into_iter()
                .map(move |root| home.join(root))
        })
        .collect()
}

/// Returns database paths that hold Claude Science conversation metadata.
///
/// An explicit `CLAUDE_SCIENCE_DB` override is exclusive: only the listed
/// files are considered, and discovery does not fall back to well-known
/// locations. Every candidate — explicit or discovered — must pass the
/// schema probe before it is read.
pub(crate) fn database_paths() -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for root in candidate_roots() {
        if root.is_file() {
            if is_claude_science_database(&root) {
                push_unique(&mut paths, root);
            }
            continue;
        }
        let mut files = Vec::new();
        collect_database_files(&root, 0, &mut files);
        files.sort();
        for path in files {
            if is_claude_science_database(&path) {
                push_unique(&mut paths, path);
            }
        }
    }
    if env::var(CLAUDE_SCIENCE_DB_ENV).is_err() {
        for path in org_database_paths() {
            if is_claude_science_database(&path) {
                push_unique(&mut paths, path);
            }
        }
    }
    Ok(paths)
}

/// Walks a candidate root looking for SQLite files, bounded to two levels so
/// a real home directory's unrelated trees are never scanned.
const MAX_SCAN_DEPTH: usize = 2;

const SKIPPED_DIRECTORY_NAMES: [&str; 3] = ["conda", "pkgs", "node_modules"];

fn collect_database_files(directory: &std::path::Path, depth: usize, files: &mut Vec<PathBuf>) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.filter_map(std::result::Result::ok) {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if file_type.is_file()
            && matches!(name.rsplit_once('.'), Some((_, ext)) if ["db", "sqlite", "sqlite3"].contains(&ext))
        {
            files.push(path);
        } else if file_type.is_dir()
            && !SKIPPED_DIRECTORY_NAMES.contains(&name.as_ref())
            && !name.starts_with('.')
        {
            collect_database_files(&path, depth + 1, files);
        }
    }
}

fn push_unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
    let canonical = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    if !paths.iter().any(|existing| {
        fs::canonicalize(existing).unwrap_or_else(|_| existing.clone()) == canonical
    }) {
        paths.push(path);
    }
}

/// A Claude Science metadata database is any SQLite file exposing the
/// `frames` table with every column the loader reads. Preparing the loader's
/// projection validates all of them at once, without relying on
/// pragma-function support.
pub(super) fn is_claude_science_database(path: &std::path::Path) -> bool {
    let Ok(connection) = sqlite::Connection::open_with_flags(
        path,
        sqlite::OpenFlags::new().with_read_only().with_no_mutex(),
    ) else {
        return false;
    };
    let Ok(mut statement) =
        connection.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'frames'")
    else {
        return false;
    };
    if statement.next().ok() != Some(sqlite::State::Row) {
        return false;
    }
    connection
        .prepare(
            "SELECT id, COALESCE(root_frame_id, id), model, input_tokens, output_tokens, \
             cache_read_tokens, cache_write_tokens, total_cost, updated_at FROM frames LIMIT 1",
        )
        .is_ok()
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use ccusage_test_support::{EnvVarsGuard, fs_fixture};

    use super::*;

    const SCHEMA: &str = "CREATE TABLE frames (
        id TEXT PRIMARY KEY,
        parent_frame_id TEXT,
        root_frame_id TEXT,
        model TEXT,
        input_tokens INTEGER,
        output_tokens INTEGER,
        cache_read_tokens INTEGER,
        cache_write_tokens INTEGER,
        total_cost REAL,
        updated_at INTEGER
    ); CREATE TABLE projects (id TEXT);";

    #[test]
    fn discovers_databases_with_frames_table() {
        let fixture = fs_fixture!({
            ".claude-science/app.db": "",
            ".claude-science/other.db": "",
        });
        let connection = sqlite::Connection::open(fixture.path(".claude-science/app.db")).unwrap();
        connection.execute(SCHEMA).unwrap();
        sqlite::Connection::open(fixture.path(".claude-science/other.db"))
            .unwrap()
            .execute("CREATE TABLE unrelated (value TEXT);")
            .unwrap();
        let _guard = EnvVarsGuard::set_many([
            (CLAUDE_SCIENCE_DB_ENV, None),
            ("HOME", Some(OsString::from(fixture.root()))),
            ("USERPROFILE", Some(OsString::from(fixture.root()))),
        ]);

        let paths = database_paths().unwrap();

        assert_eq!(paths, vec![fixture.path(".claude-science/app.db")]);
    }

    #[test]
    fn discovers_org_databases_outside_scan_depth() {
        let fixture = fs_fixture!({});
        let org = fixture
            .root()
            .join(".claude-science/cs-switch-proxy/orgs/test-org");
        let _ = std::fs::create_dir_all(&org);
        let db_path = org.join("operon-cli.db");
        sqlite::Connection::open(&db_path)
            .unwrap()
            .execute(SCHEMA)
            .unwrap();
        let _guard = EnvVarsGuard::set_many([
            (CLAUDE_SCIENCE_DB_ENV, None),
            ("HOME", Some(OsString::from(fixture.root()))),
            ("USERPROFILE", Some(OsString::from(fixture.root()))),
        ]);

        let paths = database_paths().unwrap();

        assert_eq!(paths, vec![db_path]);
    }

    #[test]
    fn env_override_is_exclusive() {
        let fixture = fs_fixture!({
            ".claude-science/override.db": "",
        });
        sqlite::Connection::open(fixture.path(".claude-science/override.db"))
            .unwrap()
            .execute(SCHEMA)
            .unwrap();
        let org = fixture
            .root()
            .join(".claude-science/cs-switch-proxy/orgs/test-org");
        let _ = std::fs::create_dir_all(&org);
        let org_db = org.join("operon-cli.db");
        sqlite::Connection::open(&org_db)
            .unwrap()
            .execute(SCHEMA)
            .unwrap();
        let _guard = EnvVarsGuard::set_many([
            (
                CLAUDE_SCIENCE_DB_ENV,
                Some(fixture.path(".claude-science/override.db").into_os_string()),
            ),
            ("HOME", Some(OsString::from(fixture.root()))),
            ("USERPROFILE", Some(OsString::from(fixture.root()))),
        ]);

        let paths = database_paths().unwrap();

        assert_eq!(paths, vec![fixture.path(".claude-science/override.db")]);
    }

    #[test]
    fn env_override_skips_incompatible_database() {
        let fixture = fs_fixture!({
            ".claude-science/custom/metadata.db": "",
        });
        let connection =
            sqlite::Connection::open(fixture.path(".claude-science/custom/metadata.db")).unwrap();
        connection.execute("CREATE TABLE frames (id TEXT)").unwrap();
        let _guard = EnvVarsGuard::set_many([
            (
                CLAUDE_SCIENCE_DB_ENV,
                Some(
                    fixture
                        .path(".claude-science/custom/metadata.db")
                        .into_os_string(),
                ),
            ),
            ("HOME", Some(OsString::from(fixture.root()))),
            ("USERPROFILE", Some(OsString::from(fixture.root()))),
        ]);

        let paths = database_paths().unwrap();

        assert!(paths.is_empty());
    }

    #[test]
    fn env_override_points_at_explicit_file() {
        let fixture = fs_fixture!({
            "custom/metadata.db": "",
        });
        sqlite::Connection::open(fixture.path("custom/metadata.db"))
            .unwrap()
            .execute(SCHEMA)
            .unwrap();
        let _guard = EnvVarsGuard::set_many([(
            CLAUDE_SCIENCE_DB_ENV,
            Some(OsString::from(
                fixture.path("custom/metadata.db").display().to_string(),
            )),
        )]);

        let paths = database_paths().unwrap();

        assert_eq!(paths, vec![fixture.path("custom/metadata.db")]);
    }
}
