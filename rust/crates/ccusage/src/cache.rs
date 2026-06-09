//! On-disk caching of parsed usage entries.
//!
//! Parsing agent log files is the dominant cost of a ccusage run, so this module
//! persists the parsed [`LoadedEntry`] values per source file; unchanged files
//! are never re-parsed.
//!
//! # Layout
//!
//! One SQLite database, `cache.db`, under [`cache_dir`] (`$XDG_CACHE_HOME/ccusage`,
//! falling back to `~/.cache/ccusage`). Three tables:
//!
//! - `files` — one row per source file: freshness fingerprint (mtime, size, cost
//!   fingerprint) and a postcard-encoded `Vec<CachedEntry>` blob.
//! - `ledger` — billable entries retained from deleted source files, keyed by
//!   `(namespace, dedup_key)`. The primary key makes a duplicate append a no-op,
//!   so spend is counted at most once and survives source-file deletion.
//! - `opencode` — one row per OpenCode message database, keyed by its cache key.
//!
//! # Validity
//!
//! A `files` row is trusted only while the source's mtime, size, and cost
//! fingerprint all match what was recorded; any drift (or a missing file) demotes
//! the path to "fresh" and the adapter re-parses it.
//!
//! # Concurrency
//!
//! Writes run inside one `BEGIN IMMEDIATE` transaction; WAL lets readers proceed
//! while a writer commits, and a busy timeout makes competing writers wait rather
//! than fail. Every database error is non-fatal: the run degrades to parsing
//! without the cache.

use std::{
    collections::{HashMap, HashSet},
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

use rustc_hash::FxHasher;
use serde::{Deserialize, Serialize};

use crate::{
    LoadedEntry, chunk_file_indexes_by_size,
    cli::CostMode,
    types::{TokenUsageRaw, UsageEntry, UsageMessage},
};

/// Lean on-disk representation of a [`LoadedEntry`].
///
/// Only the fields consumed after parsing are persisted; parse-only fields
/// (duplicate session_id/timestamp, cost_usd already folded into `.cost`,
/// is_api_error_message) are stripped and zeroed on read. Kept: `usage` (token
/// counts), `version` (version breakdowns), `message_id`/`request_id`/
/// `is_sidechain` (billable-call identity for cross-file dedup and the ledger
/// key), and `message_model` (read by some adapters post-load).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CachedEntry {
    timestamp: crate::date_utils::TimestampMs,
    date: String,
    project: Arc<str>,
    session_id: Arc<str>,
    project_path: Arc<str>,
    cost: f64,
    extra_total_tokens: u64,
    credits: Option<f64>,
    message_count: Option<u64>,
    model: Option<String>,
    usage_limit_reset_time: Option<crate::date_utils::TimestampMs>,
    missing_pricing_model: Option<String>,
    // Flattened from UsageEntry — only what is used post-parse:
    usage: TokenUsageRaw,
    version: Option<String>,
    message_id: Option<String>,
    request_id: Option<String>,
    is_sidechain: Option<bool>,
    message_model: Option<String>,
}

impl From<&LoadedEntry> for CachedEntry {
    fn from(e: &LoadedEntry) -> Self {
        CachedEntry {
            timestamp: e.timestamp,
            date: e.date.clone(),
            project: Arc::clone(&e.project),
            session_id: Arc::clone(&e.session_id),
            project_path: Arc::clone(&e.project_path),
            cost: e.cost,
            extra_total_tokens: e.extra_total_tokens,
            credits: e.credits,
            message_count: e.message_count,
            model: e.model.clone(),
            usage_limit_reset_time: e.usage_limit_reset_time,
            missing_pricing_model: e.missing_pricing_model.clone(),
            usage: e.data.message.usage,
            version: e.data.version.clone(),
            message_id: e.data.message.id.clone(),
            request_id: e.data.request_id.clone(),
            is_sidechain: e.data.is_sidechain,
            message_model: e.data.message.model.clone(),
        }
    }
}

impl From<CachedEntry> for LoadedEntry {
    fn from(c: CachedEntry) -> Self {
        LoadedEntry {
            data: UsageEntry {
                session_id: Some(c.session_id.to_string()),
                timestamp: String::new(),
                version: c.version,
                message: UsageMessage {
                    usage: c.usage,
                    model: c.message_model,
                    id: c.message_id,
                },
                cost_usd: None,
                request_id: c.request_id,
                is_api_error_message: None,
                is_sidechain: c.is_sidechain,
            },
            timestamp: c.timestamp,
            date: c.date,
            project: c.project,
            session_id: c.session_id,
            project_path: c.project_path,
            cost: c.cost,
            extra_total_tokens: c.extra_total_tokens,
            credits: c.credits,
            message_count: c.message_count,
            model: c.model,
            usage_limit_reset_time: c.usage_limit_reset_time,
            missing_pricing_model: c.missing_pricing_model,
        }
    }
}

/// Tracking-only projection retained for a *deleted* source file.
///
/// A ledger row exists to preserve spend after its source log is gone. Once the
/// source is deleted there is no future cross-file dedup to run against it, so
/// the dedup-identity fields (`request_id`, `is_sidechain`, and the blob copy of
/// `message_id`) are not persisted — only the fields that feed reports are kept.
/// The one dedup that can still matter — a deleted call reappearing in a live
/// file on resume — is resolved by the `dedup_key` primary-key column, compared
/// before the blob is ever decoded; the id is restored from that column on read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct LedgerEntry {
    timestamp: crate::date_utils::TimestampMs,
    date: String,
    project: Arc<str>,
    session_id: Arc<str>,
    project_path: Arc<str>,
    cost: f64,
    extra_total_tokens: u64,
    credits: Option<f64>,
    message_count: Option<u64>,
    model: Option<String>,
    usage_limit_reset_time: Option<crate::date_utils::TimestampMs>,
    missing_pricing_model: Option<String>,
    usage: TokenUsageRaw,
    version: Option<String>,
    message_model: Option<String>,
}

impl From<&LoadedEntry> for LedgerEntry {
    fn from(e: &LoadedEntry) -> Self {
        LedgerEntry {
            timestamp: e.timestamp,
            date: e.date.clone(),
            project: Arc::clone(&e.project),
            session_id: Arc::clone(&e.session_id),
            project_path: Arc::clone(&e.project_path),
            cost: e.cost,
            extra_total_tokens: e.extra_total_tokens,
            credits: e.credits,
            message_count: e.message_count,
            model: e.model.clone(),
            usage_limit_reset_time: e.usage_limit_reset_time,
            missing_pricing_model: e.missing_pricing_model.clone(),
            usage: e.data.message.usage,
            version: e.data.version.clone(),
            message_model: e.data.message.model.clone(),
        }
    }
}

impl LedgerEntry {
    /// Rebuild a [`LoadedEntry`] from a ledger row. `dedup_key` is the row's
    /// primary-key column; it is restored as the message id so the entry keeps
    /// the identity it had when stored (the natural id, or the synthetic key for
    /// adapters without one). The remaining dedup-only fields are never read off
    /// a deleted-source record, so they reconstruct as `None`.
    fn into_loaded(self, dedup_key: String) -> LoadedEntry {
        LoadedEntry {
            data: UsageEntry {
                session_id: Some(self.session_id.to_string()),
                timestamp: String::new(),
                version: self.version,
                message: UsageMessage {
                    usage: self.usage,
                    model: self.message_model,
                    id: Some(dedup_key),
                },
                cost_usd: None,
                request_id: None,
                is_api_error_message: None,
                is_sidechain: None,
            },
            timestamp: self.timestamp,
            date: self.date,
            project: self.project,
            session_id: self.session_id,
            project_path: self.project_path,
            cost: self.cost,
            extra_total_tokens: self.extra_total_tokens,
            credits: self.credits,
            message_count: self.message_count,
            model: self.model,
            usage_limit_reset_time: self.usage_limit_reset_time,
            missing_pricing_model: self.missing_pricing_model,
        }
    }
}

/// Freshness fingerprint for a single source file plus its cache key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FileMetadata {
    pub(crate) mtime_epoch_millis: u64,
    pub(crate) size: u64,
    /// Stable per-path key, used by the OpenCode adapter to key its row cache.
    pub(crate) cache_key: String,
    /// Fingerprint of pricing + cost-mode used when caching. Mismatch invalidates the cache.
    pub(crate) cost_fingerprint: u64,
}

/// Parsed entries recovered from the cache for one source file.
struct CachedEntries {
    entries: Vec<LoadedEntry>,
}

/// Root directory for all ccusage cache files.
pub(crate) fn cache_dir() -> Option<PathBuf> {
    Some(dirs_cache_dir()?.join("ccusage"))
}

fn dirs_cache_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CACHE_HOME") {
        return Some(PathBuf::from(dir));
    }
    if let Some(dir) = std::env::var_os("HOME") {
        return Some(PathBuf::from(dir).join(".cache"));
    }
    None
}

/// Stable cache key derived from the source file path.
///
/// `FxHasher` is fast but not collision-resistant. The cache no longer keys
/// stored entries by this value (the `files` table is keyed by the path itself),
/// so a collision is harmless here; the key remains only as a compact, stable
/// identifier for the OpenCode adapter's per-database row cache.
fn cache_key(path: &Path) -> String {
    let mut hasher = FxHasher::default();
    path.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Compute a fingerprint from pricing and cost mode for cache invalidation.
pub(crate) fn cost_fingerprint(pricing_fingerprint: Option<u64>, mode: CostMode) -> u64 {
    // Mix through a hasher rather than XOR-ing shifted fields: an XOR with a
    // shifted mode discriminant can collapse the "no pricing" (Display) domain
    // onto a real pricing hash. Hashing presence + value + mode keeps the
    // domains separate.
    let mut hasher = FxHasher::default();
    pricing_fingerprint.is_some().hash(&mut hasher);
    pricing_fingerprint.unwrap_or(0).hash(&mut hasher);
    (mode as u64).hash(&mut hasher);
    hasher.finish()
}

/// Capture the current freshness fingerprint of a source file, if it exists.
pub(crate) fn file_metadata(path: &Path) -> Option<FileMetadata> {
    let metadata = fs::metadata(path).ok()?;
    let mtime = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis()
        .min(u128::from(u64::MAX)) as u64;
    Some(FileMetadata {
        mtime_epoch_millis: mtime,
        size: metadata.len(),
        cache_key: cache_key(path),
        cost_fingerprint: 0, // filled in by caller
    })
}

// ---------------------------------------------------------------------------
// SQLite store
// ---------------------------------------------------------------------------

/// File name of the cache database within [`cache_dir`].
const DB_FILE: &str = "cache.db";
/// Schema version for the `files` table, tracked in `schema_meta`.
const FILES_SCHEMA_VERSION: i64 = 1;
/// Schema version for the `ledger` table, tracked in `schema_meta`.
const LEDGER_SCHEMA_VERSION: i64 = 1;
/// Schema version for the `opencode` table, tracked in `schema_meta`.
const OPENCODE_SCHEMA_VERSION: i64 = 1;
/// Schema version for the `pricing` table, tracked in `schema_meta`.
const PRICING_SCHEMA_VERSION: i64 = 1;

/// Encode parsed entries into the compact blob stored in the `files` table.
fn encode_entries(entries: &[LoadedEntry]) -> Option<Vec<u8>> {
    let cached: Vec<CachedEntry> = entries.iter().map(CachedEntry::from).collect();
    postcard::to_allocvec(&cached).ok()
}

/// Decode a `files` blob back into reconstructed entries.
fn decode_entries(bytes: &[u8]) -> Option<Vec<LoadedEntry>> {
    let cached: Vec<CachedEntry> = postcard::from_bytes(bytes).ok()?;
    Some(cached.into_iter().map(LoadedEntry::from).collect())
}

/// Open (creating if needed) the cache database, apply pragmas, and migrate the
/// schema. Returns `None` on any failure so the caller degrades to parsing
/// without a cache, consistent with this module's non-fatal philosophy.
fn open_db() -> Option<sqlite::Connection> {
    let dir = cache_dir()?;
    fs::create_dir_all(&dir).ok()?;
    let conn = sqlite::open(dir.join(DB_FILE)).ok()?;
    // Set the busy timeout first so ordinary lock waits — BEGIN IMMEDIATE and the
    // schema migration — block and retry instead of failing outright.
    conn.execute("PRAGMA busy_timeout=5000;").ok()?;
    // Switch to WAL so readers run while one writer commits (NORMAL stays durable
    // under WAL). The switch needs a lock upgrade that busy_timeout cannot cover,
    // so it carries its own bounded retry and never discards the connection.
    set_wal_mode(&conn);
    migrate(&conn)?;
    Some(conn)
}

/// Switch `conn` to WAL journaling, retrying past the cold-open upgrade deadlock.
///
/// Changing journal mode needs a SHARED->EXCLUSIVE lock upgrade. When several
/// processes open a fresh database at once they can each hold SHARED while all
/// want EXCLUSIVE; SQLite returns `SQLITE_BUSY` immediately for that upgrade
/// instead of invoking the busy handler (calling it would only deadlock), so
/// `busy_timeout` cannot cover this case. A short bounded backoff resolves it:
/// once any opener wins the switch the file is WAL for everyone, so a loser's
/// retry is a no-op success. If every attempt loses — pathologically unlikely —
/// the connection stays in its default rollback journal; writes still serialize
/// through `busy_timeout`, so no row is lost, only cross-process read concurrency
/// is reduced until a later uncontended run flips it to WAL.
fn set_wal_mode(conn: &sqlite::Connection) {
    for attempt in 0..8u64 {
        if conn
            .execute("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .is_ok()
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(attempt + 1));
    }
}
/// Remove the entire cache database and its sidecars. Non-fatal: all IO
/// errors are ignored and there is no output. If the cache directory does not
/// resolve, this is a no-op.
pub(crate) fn clear_cache() {
    let Some(dir) = cache_dir() else {
        return;
    };
    for name in &[DB_FILE, "cache.db-wal", "cache.db-shm"] {
        let _ = fs::remove_file(dir.join(name));
    }
}

/// Remove the `files` and `ledger` rows for `agent`'s namespaces, plus the
/// `opencode` table when the agent is "opencode". Returns whether `agent` is
/// cacheable, so callers can report a no-op for agents with no on-disk cache.
/// Non-fatal: all IO/SQL errors are ignored.
pub(crate) fn clear_cache_namespaces(agent: &str) -> bool {
    let namespaces = agent_namespaces(agent);
    let cacheable = !namespaces.is_empty();
    let Some(conn) = open_db() else {
        return cacheable;
    };
    for ns in namespaces {
        if let Ok(mut st) = conn.prepare("DELETE FROM files WHERE namespace = ?") {
            let _ = st.bind((1, *ns));
            let _ = st.next();
        }
        if let Ok(mut st) = conn.prepare("DELETE FROM ledger WHERE namespace = ?") {
            let _ = st.bind((1, *ns));
            let _ = st.next();
        }
    }
    if agent == "opencode" {
        let _ = conn.execute("DELETE FROM opencode");
    }
    cacheable
}

fn agent_namespaces(agent: &str) -> &'static [&'static str] {
    match agent {
        "claude" => &["claude"],
        "opencode" => &["opencode", "opencode-db"],
        "amp" => &["amp"],
        "copilot" => &["copilot"],
        "droid" => &["droid"],
        "gemini" => &["gemini"],
        "kilo" => &["kilo"],
        "openclaw" => &["openclaw"],
        "pi" => &["pi"],
        "qwen" => &["qwen"],
        _ => &[],
    }
}

/// Create the schema if absent and stamp each table's version into `schema_meta`.
///
/// Only `ledger` is irreplaceable; the regenerable tables self-heal via their
/// freshness fingerprints. A stale ledger version may no longer decode, so it is
/// surfaced rather than dropped silently — see the `FUTURE` marker for migration.
fn migrate(conn: &sqlite::Connection) -> Option<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS files (\
             path TEXT PRIMARY KEY,\
             namespace TEXT NOT NULL,\
             mtime INTEGER NOT NULL,\
             size INTEGER NOT NULL,\
             cost_fingerprint INTEGER NOT NULL,\
             entries BLOB NOT NULL\
         );\
         CREATE INDEX IF NOT EXISTS files_namespace ON files(namespace);\
         CREATE TABLE IF NOT EXISTS ledger (\
             namespace TEXT NOT NULL,\
             dedup_key TEXT NOT NULL,\
             entry BLOB NOT NULL,\
             PRIMARY KEY (namespace, dedup_key)\
         );\
         CREATE TABLE IF NOT EXISTS opencode (\
             db_key TEXT PRIMARY KEY,\
             cost_fingerprint INTEGER NOT NULL,\
             rows BLOB NOT NULL\
         );\
         CREATE TABLE IF NOT EXISTS pricing (\
             url TEXT PRIMARY KEY,\
             etag TEXT,\
             last_modified TEXT,\
             body TEXT NOT NULL\
        );\
        CREATE TABLE IF NOT EXISTS schema_meta (\
             name TEXT PRIMARY KEY,\
             version INTEGER NOT NULL\
         );",
    )
    .ok()?;
    {
        let mut st = conn
            .prepare(
                "INSERT OR IGNORE INTO schema_meta(name, version) VALUES\
                     (?, ?), (?, ?), (?, ?), (?, ?)",
            )
            .ok()?;
        st.bind((1, "files")).ok()?;
        st.bind((2, FILES_SCHEMA_VERSION)).ok()?;
        st.bind((3, "ledger")).ok()?;
        st.bind((4, LEDGER_SCHEMA_VERSION)).ok()?;
        st.bind((5, "opencode")).ok()?;
        st.bind((6, OPENCODE_SCHEMA_VERSION)).ok()?;
        st.bind((7, "pricing")).ok()?;
        st.bind((8, PRICING_SCHEMA_VERSION)).ok()?;
        st.next().ok()?;
    }
    if let Some(stored) = read_schema_version(conn, "ledger")
        && stored != LEDGER_SCHEMA_VERSION
    {
        eprintln!(
            "WARN  Ledger cache schema v{stored} differs from expected v{LEDGER_SCHEMA_VERSION}; \
             retained spend may be dropped. Run `ccusage clear-cache` if totals look wrong."
        );
    }
    // FUTURE: migration logic — when a table's on-disk layout changes
    // incompatibly, bump its constant above and add per-table migration here.
    Some(())
}

/// Read a table's stored schema version, or `None` if unstamped/unreadable.
fn read_schema_version(conn: &sqlite::Connection, name: &str) -> Option<i64> {
    let mut st = conn
        .prepare("SELECT version FROM schema_meta WHERE name = ?")
        .ok()?;
    st.bind((1, name)).ok()?;
    match st.next() {
        Ok(sqlite::State::Row) => st.read::<i64, _>(0).ok(),
        _ => None,
    }
}

/// A `files` row's freshness fingerprint and its (still encoded) entries blob.
struct StoredFile {
    mtime: u64,
    size: u64,
    cost_fingerprint: u64,
    entries: Vec<u8>,
}

/// Load every `files` row for `namespace`, keyed by source path. The entries
/// blob is decoded lazily by [`partition_files`] only on a confirmed cache hit.
fn load_namespace_files(conn: &sqlite::Connection, namespace: &str) -> HashMap<String, StoredFile> {
    let mut map = HashMap::new();
    let Ok(mut st) = conn.prepare(
        "SELECT path, mtime, size, cost_fingerprint, entries FROM files WHERE namespace = ?",
    ) else {
        return map;
    };
    if st.bind((1, namespace)).is_err() {
        return map;
    }
    while let Ok(sqlite::State::Row) = st.next() {
        let (Ok(path), Ok(mtime), Ok(size), Ok(cost), Ok(entries)) = (
            st.read::<String, _>(0),
            st.read::<i64, _>(1),
            st.read::<i64, _>(2),
            st.read::<i64, _>(3),
            st.read::<Vec<u8>, _>(4),
        ) else {
            continue;
        };
        map.insert(
            path,
            StoredFile {
                mtime: mtime as u64,
                size: size as u64,
                cost_fingerprint: cost as u64,
                entries,
            },
        );
    }
    map
}

/// A source file that must be re-parsed, paired with the freshness fingerprint
/// captured *before* the parse. Caching against this pre-parse metadata (rather
/// than re-statting afterwards) guarantees that a concurrent append during the
/// parse can never be recorded as already-cached: the post-append file will
/// mismatch the stored size/mtime on the next run and be re-parsed.
struct FreshFile {
    path: PathBuf,
    /// `None` when the file vanished or could not be statted; such a file is
    /// parsed but never cached.
    metadata: Option<FileMetadata>,
}

/// The split of an input file list into cache hits and files needing parsing.
struct FilePartition {
    cached: Vec<CachedEntries>,
    fresh: Vec<FreshFile>,
}

/// Partition source files into cached (still valid) and fresh (must re-parse).
///
/// `cost_fingerprint` is a hash of the pricing + cost-mode used at parse time.
/// Cached entries are invalidated when this fingerprint changes.
fn partition_files(
    files: &[PathBuf],
    stored: &HashMap<String, StoredFile>,
    cost_fingerprint: u64,
) -> FilePartition {
    let mut cached = Vec::new();
    let mut fresh = Vec::new();

    for path in files {
        let Some(current) = file_metadata(path) else {
            fresh.push(FreshFile {
                path: path.clone(),
                metadata: None,
            });
            continue;
        };

        let path_str = path.to_string_lossy().to_string();
        if let Some(entry) = stored.get(&path_str) {
            let unchanged = entry.mtime == current.mtime_epoch_millis
                && entry.size == current.size
                && entry.cost_fingerprint == cost_fingerprint;
            if unchanged {
                if let Some(entries) = decode_entries(&entry.entries) {
                    cached.push(CachedEntries { entries });
                    continue;
                }
            }
        }

        let mut current = current;
        current.cost_fingerprint = cost_fingerprint;
        fresh.push(FreshFile {
            path: path.clone(),
            metadata: Some(current),
        });
    }

    FilePartition { cached, fresh }
}

/// Load parsed entries for `files`, reusing the cache for unchanged files and
/// parsing only the fresh ones with `parse_file`. `namespace` identifies the
/// calling adapter (e.g. `"claude"`, `"amp"`).
///
/// Every billable live entry is recorded in the ledger so its spend persists
/// after its source file is deleted; ledger entries whose source is gone are
/// re-emitted, while those still on disk are skipped (the live copy wins via the
/// adapter's dedup). When `live_only` is set the re-emission is suppressed, so
/// the result holds only entries whose source file still exists on disk.
///
/// `parse_file` returns `Err` when a file cannot be read or parsed; the error
/// propagates and a failed run is never persisted.
pub(crate) fn load_with_cache<F>(
    namespace: &str,
    files: &[PathBuf],
    single_thread: bool,
    cost_fingerprint: u64,
    live_only: bool,
    parse_file: F,
) -> crate::Result<Vec<LoadedEntry>>
where
    F: Fn(&Path) -> crate::Result<Vec<LoadedEntry>> + Sync,
{
    let conn = open_db();

    // Partition against the stored namespace snapshot (empty when the cache is
    // unavailable, which forces every file to be parsed fresh).
    let stored = conn
        .as_ref()
        .map(|conn| load_namespace_files(conn, namespace))
        .unwrap_or_default();
    let partition = partition_files(files, &stored, cost_fingerprint);

    let fresh_paths: Vec<PathBuf> = partition.fresh.iter().map(|f| f.path.clone()).collect();
    let mut parsed = parse_fresh_files(&fresh_paths, single_thread, &parse_file)?;

    // Reprice fresh entries before persisting so cache and ledger store current cost.
    for entry in parsed.iter_mut().flatten() {
        reprice(entry);
    }

    // Persist fresh entries, append them to the ledger, and prune deleted sources
    // in one transaction. Skipped when the cache is unavailable.
    if let Some(conn) = conn.as_ref() {
        write_back(conn, namespace, &partition.fresh, &parsed);
    }

    // Assemble live entries (cached hits first, then freshly parsed).
    let cached_total: usize = partition.cached.iter().map(|c| c.entries.len()).sum();
    let fresh_total: usize = parsed.iter().map(Vec::len).sum();
    let mut live = Vec::with_capacity(cached_total + fresh_total);
    for cached in partition.cached {
        live.extend(cached.entries);
    }
    for entries in parsed {
        live.extend(entries);
    }

    // Reprice cached hits; fresh entries were already repriced above.
    for entry in &mut live[..cached_total] {
        reprice(entry);
    }

    // Merge the ledger: record new billable entries and re-emit entries whose
    // source file has since been deleted. Without a cache there is nothing to
    // merge, so the live entries pass through unchanged.
    Ok(match conn {
        Some(conn) => merge_ledger(&conn, namespace, live, live_only),
        None => live,
    })
}

/// Upsert fresh entries, append them to the ledger, and prune deleted sources in
/// one transaction. On failure it rolls back, so an uncached file is re-parsed and
/// re-appended next run.
fn write_back(
    conn: &sqlite::Connection,
    namespace: &str,
    fresh: &[FreshFile],
    parsed: &[Vec<LoadedEntry>],
) {
    if conn.execute("BEGIN IMMEDIATE").is_err() {
        return;
    }
    let committed = (|| -> Option<()> {
        for (fresh, entries) in fresh.iter().zip(parsed.iter()) {
            // Cache against the pre-parse fingerprint so a concurrent append is
            // re-parsed next run instead of being silently trusted as cached. A
            // file that vanished mid-run has no metadata and is left uncached.
            if let Some(meta) = &fresh.metadata {
                upsert_file(conn, namespace, &fresh.path, meta, entries)?;
            }
            append_entries_to_ledger(conn, namespace, entries)?;
        }
        prune_deleted(conn, namespace)?;
        Some(())
    })();
    let _ = conn.execute(if committed.is_some() {
        "COMMIT"
    } else {
        "ROLLBACK"
    });
}

/// Insert or replace the `files` row for one source file.
fn upsert_file(
    conn: &sqlite::Connection,
    namespace: &str,
    path: &Path,
    meta: &FileMetadata,
    entries: &[LoadedEntry],
) -> Option<()> {
    let blob = encode_entries(entries)?;
    let mut st = conn
        .prepare(
            "INSERT OR REPLACE INTO files(path, namespace, mtime, size, cost_fingerprint, entries) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .ok()?;
    let path_str = path.to_string_lossy();
    st.bind((1, path_str.as_ref())).ok()?;
    st.bind((2, namespace)).ok()?;
    st.bind((3, meta.mtime_epoch_millis as i64)).ok()?;
    st.bind((4, meta.size as i64)).ok()?;
    st.bind((5, meta.cost_fingerprint as i64)).ok()?;
    st.bind((6, &blob[..])).ok()?;
    st.next().ok()?;
    Some(())
}

/// Drop `files` rows for `namespace` whose source file no longer exists on disk.
/// Their spend is preserved in the ledger (appended while the file was last
/// live), so the cached entries for a deleted file are dead weight; removing
/// them keeps the cache bounded.
fn prune_deleted(conn: &sqlite::Connection, namespace: &str) -> Option<()> {
    let mut gone = Vec::new();
    {
        let mut st = conn
            .prepare("SELECT path FROM files WHERE namespace = ?")
            .ok()?;
        st.bind((1, namespace)).ok()?;
        while let Ok(sqlite::State::Row) = st.next() {
            if let Ok(path) = st.read::<String, _>(0) {
                if file_metadata(Path::new(&path)).is_none() {
                    gone.push(path);
                }
            }
        }
    }
    for path in gone {
        let mut st = conn.prepare("DELETE FROM files WHERE path = ?").ok()?;
        st.bind((1, path.as_str())).ok()?;
        st.next().ok()?;
    }
    Some(())
}

/// One cached OpenCode message-database row: its id, a content hash for
/// freshness validation, and the parsed entry. The content hash is computed
/// from the `data` string so changes to the row content (without time_updated
/// changing) correctly invalidate the cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct OpenCodeRow {
    pub(crate) id: String,
    /// Hash of the `data` field for cache validity checking.
    pub(crate) content_hash: u64,
    pub(crate) entry: CachedEntry,
}

/// Load the cached rows for an OpenCode database, keyed by [`file_metadata`]'s
/// `cache_key`. Returns `None` on any missing/corrupt cache or fingerprint
/// mismatch, mirroring the rest of this module's tolerance for stale or
/// unreadable cache state.
pub(crate) fn load_opencode_row_cache(
    cache_key: &str,
    cost_fingerprint: u64,
) -> Option<Vec<OpenCodeRow>> {
    let conn = open_db()?;
    let mut st = conn
        .prepare("SELECT cost_fingerprint, rows FROM opencode WHERE db_key = ?")
        .ok()?;
    st.bind((1, cache_key)).ok()?;
    match st.next().ok()? {
        sqlite::State::Row => {
            if st.read::<i64, _>(0).ok()? as u64 != cost_fingerprint {
                return None;
            }
            let blob = st.read::<Vec<u8>, _>(1).ok()?;
            postcard::from_bytes::<Vec<OpenCodeRow>>(&blob).ok()
        }
        sqlite::State::Done => None,
    }
}

/// Persist the cached rows for an OpenCode database. Failures are silently
/// ignored: a missing cache simply forces a full re-parse next run.
pub(crate) fn save_opencode_row_cache(
    cache_key: &str,
    rows: &[OpenCodeRow],
    cost_fingerprint: u64,
) {
    let Some(conn) = open_db() else { return };
    let Ok(blob) = postcard::to_allocvec(rows) else {
        return;
    };
    let Ok(mut st) = conn.prepare(
        "INSERT OR REPLACE INTO opencode(db_key, cost_fingerprint, rows) VALUES (?, ?, ?)",
    ) else {
        return;
    };
    let _ = st.bind((1, cache_key));
    let _ = st.bind((2, cost_fingerprint as i64));
    let _ = st.bind((3, &blob[..]));
    let _ = st.next();
}
/// Cached conditional-fetch state for a remote pricing JSON document.
pub(crate) struct CachedPricing {
    pub body: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// Load cached pricing metadata for `url`. Returns `None` on any miss or
/// error — a missing cache simply forces an unconditional fetch.
pub(crate) fn load_pricing(url: &str) -> Option<CachedPricing> {
    let conn = open_db()?;
    let mut st = conn
        .prepare("SELECT etag, last_modified, body FROM pricing WHERE url = ?")
        .ok()?;
    st.bind((1, url)).ok()?;
    match st.next().ok()? {
        sqlite::State::Row => Some(CachedPricing {
            etag: st.read::<Option<String>, _>(0).ok().flatten(),
            last_modified: st.read::<Option<String>, _>(1).ok().flatten(),
            body: st.read::<String, _>(2).ok()?,
        }),
        sqlite::State::Done => None,
    }
}

/// Persist pricing metadata for `url`. Best-effort: all IO errors are
/// silently ignored (the cache is an optimization, not correctness).
pub(crate) fn store_pricing(
    url: &str,
    body: &str,
    etag: Option<&str>,
    last_modified: Option<&str>,
) {
    let Some(conn) = open_db() else { return };
    let Ok(mut st) = conn.prepare(
        "INSERT OR REPLACE INTO pricing(url, etag, last_modified, body) \
         VALUES (?, ?, ?, ?)",
    ) else {
        return;
    };
    let _ = st.bind((1, url));
    let _ = st.bind((2, etag));
    let _ = st.bind((3, last_modified));
    let _ = st.bind((4, body));
    let _ = st.next();
}

/// Returns a stable dedup key for `e` suitable for ledger writes.
///
/// Prefers the natural `message_id` (the Claude-API call id). Adapters that emit
/// entries without one (Pi, Qwen, …) get a synthetic key spanning every dimension
/// their live dedup uses — project, model, and each token bucket — so two
/// distinct calls sharing a session, timestamp, and input/output counts are not
/// collapsed into one (which would undercount spend once the source is deleted).
/// Uses token counts, not cost, so the key is invariant across repricing.
fn entry_ledger_key(e: &LoadedEntry) -> String {
    if let Some(id) = &e.data.message.id {
        return id.clone();
    }
    let usage = &e.data.message.usage;
    format!(
        "synth:{}:{}:{}:{}:{}:{}:{}:{}:{}",
        e.timestamp.as_millis(),
        e.project.as_ref(),
        e.session_id.as_ref(),
        e.model.as_deref().unwrap_or_default(),
        usage.input_tokens,
        usage.output_tokens,
        usage.cache_creation_input_tokens,
        usage.cache_read_input_tokens,
        e.extra_total_tokens,
    )
}

/// Merge the ledger with this run's live entries for `namespace`.
///
/// INVARIANT: callers pass the *full* on-disk source set for the namespace, so a
/// ledger entry whose key is absent from the live set is treated as deleted and
/// re-emitted. (A strict subset would wrongly re-emit out-of-scope entries; no
/// caller does that.)
///
/// Re-emits ledger entries whose source is gone this run; keys present in `live`
/// are emitted once from `live` (the re-priced copy wins). Live entries are
/// recorded at write-back time, not here.
///
/// `live_only` skips the re-emission, so deleted sources contribute nothing to
/// the result while their spend stays retained in the ledger for a later run.
fn merge_ledger(
    conn: &sqlite::Connection,
    namespace: &str,
    live: Vec<LoadedEntry>,
    live_only: bool,
) -> Vec<LoadedEntry> {
    let mut seen: HashSet<String> = live.iter().map(entry_ledger_key).collect();
    let mut out = live;

    if !live_only {
        // Read keys before blobs: a key missing from the live set marks a deleted
        // source whose blob must be decoded, while a key already covered by `out`
        // is skipped — so a warm run with no deletions decodes no ledger blobs.
        let deleted_keys: Vec<String> = load_ledger_keys(conn, namespace)
            .into_iter()
            .filter(|k| !seen.contains(k))
            .collect();
        for key in deleted_keys {
            if let Some(blob) = load_ledger_blob(conn, namespace, &key) {
                if let Ok(entry) = postcard::from_bytes::<LedgerEntry>(&blob) {
                    seen.insert(key.clone());
                    out.push(entry.into_loaded(key));
                }
            }
        }
    }
    out
}

/// Append `entries` to the ledger in the caller's transaction. The primary key
/// makes inserts idempotent, so spend is recorded at most once.
fn append_entries_to_ledger(
    conn: &sqlite::Connection,
    namespace: &str,
    entries: &[LoadedEntry],
) -> Option<()> {
    let mut st = conn
        .prepare(
            "INSERT OR IGNORE INTO ledger(namespace, dedup_key, entry) \
             VALUES (?, ?, ?)",
        )
        .ok()?;
    for e in entries {
        let key = entry_ledger_key(e);
        // The dedup key is the row's primary-key column, so the blob omits every
        // dedup-identity field; the id is restored from the column on read.
        let blob = postcard::to_allocvec(&LedgerEntry::from(e)).ok()?;
        st.reset().ok()?;
        st.bind((1, namespace)).ok()?;
        st.bind((2, key.as_str())).ok()?;
        st.bind((3, &blob[..])).ok()?;
        st.next().ok()?;
    }
    Some(())
}

/// Load every ledger dedup key for `namespace` without decoding the entry blobs.
fn load_ledger_keys(conn: &sqlite::Connection, namespace: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(mut st) = conn.prepare("SELECT dedup_key FROM ledger WHERE namespace = ?") else {
        return out;
    };
    if st.bind((1, namespace)).is_err() {
        return out;
    }
    while let Ok(sqlite::State::Row) = st.next() {
        if let Ok(key) = st.read::<String, _>(0) {
            out.push(key);
        }
    }
    out
}

/// Fetch the postcard-encoded entry blob for one `(namespace, dedup_key)`, or
/// `None` if the row is missing.
fn load_ledger_blob(
    conn: &sqlite::Connection,
    namespace: &str,
    dedup_key: &str,
) -> Option<Vec<u8>> {
    let mut st = conn
        .prepare("SELECT entry FROM ledger WHERE namespace = ? AND dedup_key = ?")
        .ok()?;
    st.bind((1, namespace)).ok()?;
    st.bind((2, dedup_key)).ok()?;
    if let Ok(sqlite::State::Row) = st.next() {
        return st.read::<Vec<u8>, _>(0).ok();
    }
    None
}

/// Retain `live` entries for `namespace` through the ledger for sources that do
/// not flow through [`load_with_cache`] (e.g. the OpenCode SQLite database,
/// which is read directly rather than from per-file caches).
///
/// Pass the *full* current live set for the namespace — empty when the source is
/// gone — so deleted spend is re-emitted from the ledger exactly once. Use a
/// namespace distinct from any [`load_with_cache`] caller so the two live sets
/// never appear to "delete" each other. `live_only` carries the same meaning as
/// in [`load_with_cache`]: when set, retained spend for gone sources is suppressed.
pub(crate) fn retain_via_ledger(
    namespace: &str,
    live: Vec<LoadedEntry>,
    live_only: bool,
) -> Vec<LoadedEntry> {
    match open_db() {
        Some(conn) => {
            // No per-file cache here, so append the full live set in its own transaction.
            if !live.is_empty() && conn.execute("BEGIN IMMEDIATE").is_ok() {
                let ok = append_entries_to_ledger(&conn, namespace, &live).is_some();
                let _ = conn.execute(if ok { "COMMIT" } else { "ROLLBACK" });
            }
            merge_ledger(&conn, namespace, live, live_only)
        }
        None => live,
    }
}

/// Parse each fresh path with `parse_file`, returning results aligned 1:1 with
/// `fresh`. Uses size-balanced parallel chunks when worthwhile.
fn parse_fresh_files<F>(
    fresh: &[PathBuf],
    single_thread: bool,
    parse_file: &F,
) -> crate::Result<Vec<Vec<LoadedEntry>>>
where
    F: Fn(&Path) -> crate::Result<Vec<LoadedEntry>> + Sync,
{
    let worker_count = if single_thread {
        1
    } else {
        std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .min(fresh.len())
    };
    if worker_count <= 1 {
        return fresh.iter().map(|path| parse_file(path)).collect();
    }

    let chunks = chunk_file_indexes_by_size(fresh, worker_count);
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            handles.push(scope.spawn(move || {
                chunk
                    .into_iter()
                    .map(|index| (index, parse_file(&fresh[index])))
                    .collect::<Vec<_>>()
            }));
        }
        // `slot` distinguishes "worker never filled this index" (outer None, a
        // bug) from "file errored" (inner Err). We flatten the outer layer
        // away after asserting every slot was filled.
        let mut results: Vec<Option<crate::Result<Vec<LoadedEntry>>>> =
            Vec::with_capacity(fresh.len());
        results.resize_with(fresh.len(), || None);
        for (index, entries) in handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("cache parse worker panicked"))
        {
            results[index] = Some(entries);
        }
        // Find the first error, otherwise collect the successful parses.
        let mut first_err: Option<crate::CliError> = None;
        let mut vec_results: Vec<Vec<LoadedEntry>> = Vec::with_capacity(fresh.len());
        for slot in results.into_iter() {
            let slot = slot.expect("cache parse worker returned every file");
            match slot {
                Ok(entries) => vec_results.push(entries),
                Err(err) => {
                    if first_err.is_none() {
                        first_err = Some(err);
                    }
                }
            }
        }
        match first_err {
            Some(err) => Err(err),
            None => Ok(vec_results),
        }
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::{Arc, MutexGuard};

    use super::*;
    use crate::date_utils::TimestampMs;
    use crate::types::{TokenUsageRaw, UsageEntry, UsageMessage};

    /// Isolate cache I/O in a temp dir so tests never touch the real cache.
    pub(crate) struct CacheEnv {
        _dir: PathBuf,
        prev_xdg: Option<std::ffi::OsString>,
        _guard: MutexGuard<'static, ()>,
    }

    impl CacheEnv {
        pub(crate) fn new(name: &str) -> Self {
            let guard = crate::test_env_lock();
            let dir = std::env::temp_dir().join(format!("ccusage-cache-test-{name}"));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            let prev_xdg = std::env::var_os("XDG_CACHE_HOME");
            unsafe { std::env::set_var("XDG_CACHE_HOME", &dir) };
            Self {
                _dir: dir,
                prev_xdg,
                _guard: guard,
            }
        }
    }

    impl Drop for CacheEnv {
        fn drop(&mut self) {
            match &self.prev_xdg {
                Some(v) => unsafe { std::env::set_var("XDG_CACHE_HOME", v) },
                None => unsafe { std::env::remove_var("XDG_CACHE_HOME") },
            }
            let _ = fs::remove_dir_all(&self._dir);
        }
    }

    fn sample_entry() -> LoadedEntry {
        LoadedEntry {
            data: UsageEntry {
                session_id: Some("session-a".to_string()),
                timestamp: "2026-01-01T00:00:00Z".to_string(),
                version: None,
                message: UsageMessage {
                    usage: TokenUsageRaw::default(),
                    model: Some("claude-test".to_string()),
                    id: Some("msg-a".to_string()),
                },
                cost_usd: None,
                request_id: Some("req-a".to_string()),
                is_api_error_message: None,
                is_sidechain: None,
            },
            timestamp: TimestampMs::from_millis(1_000),
            date: "2026-01-01".to_string(),
            project: Arc::from("proj"),
            session_id: Arc::from("session-a"),
            project_path: Arc::from("/tmp/proj"),
            cost: 0.0,
            extra_total_tokens: 0,
            credits: None,
            message_count: None,
            model: Some("claude-test".to_string()),
            usage_limit_reset_time: None,
            missing_pricing_model: None,
        }
    }

    fn write_source(name: &str, contents: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("ccusage-src-{name}.jsonl"));
        fs::write(&path, contents).unwrap();
        path
    }

    /// Build a one-file stored snapshot mirroring what a cold run would persist,
    /// so [`partition_files`] can be exercised directly.
    fn stored_snapshot(
        path: &Path,
        meta: &FileMetadata,
        entries: &[LoadedEntry],
        cost_fingerprint: u64,
    ) -> HashMap<String, StoredFile> {
        let mut map = HashMap::new();
        map.insert(
            path.to_string_lossy().to_string(),
            StoredFile {
                mtime: meta.mtime_epoch_millis,
                size: meta.size,
                cost_fingerprint,
                entries: encode_entries(entries).unwrap(),
            },
        );
        map
    }

    /// Source paths recorded in the `files` table for `namespace`.
    fn file_paths(namespace: &str) -> HashSet<String> {
        let conn = open_db().unwrap();
        let mut st = conn
            .prepare("SELECT path FROM files WHERE namespace = ?")
            .unwrap();
        st.bind((1, namespace)).unwrap();
        let mut out = HashSet::new();
        while let Ok(sqlite::State::Row) = st.next() {
            out.insert(st.read::<String, _>(0).unwrap());
        }
        out
    }

    /// Insert a raw ledger row, used to simulate duplicate records that the
    /// production path would never write itself.
    fn insert_ledger_row(namespace: &str, key: &str, entry: &LoadedEntry) {
        let conn = open_db().unwrap();
        let blob = postcard::to_allocvec(&LedgerEntry::from(entry)).unwrap();
        let mut st = conn
            .prepare(
                "INSERT OR IGNORE INTO ledger(namespace, dedup_key, entry) \
                 VALUES (?, ?, ?)",
            )
            .unwrap();
        st.bind((1, namespace)).unwrap();
        st.bind((2, key)).unwrap();
        st.bind((3, &blob[..])).unwrap();
        st.next().unwrap();
    }

    #[test]
    fn round_trips_cached_entries_for_unchanged_file() {
        let _env = CacheEnv::new("roundtrip");
        let src = write_source("roundtrip", "line\n");
        let meta = file_metadata(&src).expect("metadata");
        let stored = stored_snapshot(&src, &meta, &[sample_entry()], 0);

        let part = partition_files(std::slice::from_ref(&src), &stored, 0);
        assert_eq!(part.cached.len(), 1);
        assert!(part.fresh.is_empty());
        assert_eq!(part.cached[0].entries[0].session_id.as_ref(), "session-a");
        let _ = fs::remove_file(&src);
    }

    #[test]
    fn invalidates_cache_when_file_changes() {
        let _env = CacheEnv::new("invalidate");
        let src = write_source("invalidate", "line\n");
        let meta = file_metadata(&src).expect("metadata");
        let stored = stored_snapshot(&src, &meta, &[sample_entry()], 0);

        // Mutate the source so its size no longer matches the fingerprint.
        fs::write(&src, "line\nlonger\n").unwrap();

        let part = partition_files(std::slice::from_ref(&src), &stored, 0);
        assert!(part.cached.is_empty());
        assert_eq!(part.fresh.len(), 1);
        assert_eq!(part.fresh[0].path, src);
        let _ = fs::remove_file(&src);
    }

    #[test]
    fn load_with_cache_serves_unchanged_files_from_cache() {
        let _env = CacheEnv::new("load-with-cache-hit");
        let src = write_source("load-with-cache-hit", "line\n");

        let calls = std::sync::atomic::AtomicUsize::new(0);
        let cold = load_with_cache(
            "test",
            std::slice::from_ref(&src),
            false,
            0,
            false,
            |_path| {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(vec![sample_entry()])
            },
        )
        .unwrap();
        assert_eq!(cold.len(), 1);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);

        let warm = load_with_cache(
            "test",
            std::slice::from_ref(&src),
            false,
            0,
            false,
            |_path| {
                panic!("parse_file should not run for a cached file");
            },
        )
        .unwrap();
        assert_eq!(warm.len(), 1);
        assert_eq!(warm[0].session_id.as_ref(), "session-a");

        let _ = fs::remove_file(&src);
    }

    #[test]
    fn load_with_cache_does_not_cache_errored_files() {
        let _env = CacheEnv::new("load-with-cache-error");
        let src = write_source("load-with-cache-error", "line\n");

        // First run: parse_file errors for an unchanged file. load_with_cache
        // propagates the error and caches nothing, so a later run re-parses
        // instead of serving a poisoned empty entry forever.
        let errored = load_with_cache(
            "test",
            std::slice::from_ref(&src),
            false,
            0,
            false,
            |path| Err(crate::cli_error(format!("boom: {}", path.display()))),
        );
        assert!(errored.is_err());

        // Second run on the same unchanged file must re-parse, not serve a
        // poisoned empty cache entry.
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let recovered = load_with_cache(
            "test",
            std::slice::from_ref(&src),
            false,
            0,
            false,
            |_path| {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(vec![sample_entry()])
            },
        )
        .unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(recovered.len(), 1);

        let _ = fs::remove_file(&src);
    }

    #[test]
    fn partitions_into_cached_and_fresh() {
        let _env = CacheEnv::new("partition");

        let cached_src = write_source("partition-cached", "a\n");
        let cached_meta = file_metadata(&cached_src).unwrap();
        let stored = stored_snapshot(&cached_src, &cached_meta, &[sample_entry()], 0);

        let fresh_src = write_source("partition-fresh", "b\n");

        let part = partition_files(&[cached_src.clone(), fresh_src.clone()], &stored, 0);

        assert_eq!(part.cached.len(), 1);
        assert_eq!(part.fresh.len(), 1);
        assert_eq!(part.fresh[0].path, fresh_src);

        let _ = fs::remove_file(&cached_src);
        let _ = fs::remove_file(&fresh_src);
    }

    // -------------------------------------------------------------------------
    // Ledger retention tests
    // -------------------------------------------------------------------------

    /// Retain-on-delete: cold-load a fixture file (cached); delete the source
    /// file; the warm `load_with_cache` still returns its entries (now from the
    /// ledger) and the deleted file's `files` row is pruned.
    #[test]
    fn retain_on_delete_ledger_emits_entries_after_source_deleted() {
        let _env = CacheEnv::new("retain-on-delete");
        let src = write_source("retain-on-delete", "line\n");

        let cold = load_with_cache(
            "test-ns",
            std::slice::from_ref(&src),
            false,
            0,
            false,
            |_| Ok(vec![sample_entry()]),
        )
        .unwrap();
        assert_eq!(cold.len(), 1);

        // Delete the source file — simulates "deleted chat".
        fs::remove_file(&src).unwrap();

        let warm = load_with_cache("test-ns", &[], false, 0, false, |_| Ok(Vec::new())).unwrap();

        assert_eq!(warm.len(), 1, "ledger entry must be emitted after deletion");
        assert_eq!(warm[0].session_id.as_ref(), "session-a");

        assert!(
            !file_paths("test-ns").contains(&src.to_string_lossy().to_string()),
            "files row must be removed after deletion"
        );
    }

    /// Retention for entries without a natural `message_id`.
    ///
    /// Adapters such as Pi and Qwen emit entries with `message_id = None`.
    /// A synthetic key must be assigned on ledger write so spend survives
    /// source-file deletion — previously these entries were silently dropped.
    #[test]
    fn retain_on_delete_no_message_id_entry() {
        let _env = CacheEnv::new("retain-no-id");
        let src = write_source("retain-no-id", "line\n");

        let mut entry = sample_entry();
        entry.data.message.id = None;
        entry.cost = 0.05;

        let cold = load_with_cache(
            "test-ns-noid",
            std::slice::from_ref(&src),
            false,
            0,
            false,
            |_| Ok(vec![entry.clone()]),
        )
        .unwrap();
        assert_eq!(cold.len(), 1);

        // Delete the source file — simulates adapter log removal.
        fs::remove_file(&src).unwrap();

        let warm =
            load_with_cache("test-ns-noid", &[], false, 0, false, |_| Ok(Vec::new())).unwrap();

        assert_eq!(
            warm.len(),
            1,
            "entry with no message_id must be re-emitted via synthetic ledger key after deletion"
        );
        assert_eq!(warm[0].session_id.as_ref(), "session-a");
        assert!(
            (warm[0].cost - 0.05).abs() < 1e-9,
            "cost must be preserved through synthetic-key ledger round-trip"
        );
    }

    /// Two synthetic-keyed entries that share session, timestamp, and
    /// input/output counts but differ in model must NOT collapse in the ledger.
    /// The widened synthetic key spans model and every token bucket, so both
    /// survive source deletion; a coarser key would silently drop one's spend.
    #[test]
    fn synthetic_key_separates_distinct_models() {
        let _env = CacheEnv::new("synth-distinct-models");
        let src = write_source("synth-distinct-models", "line\n");

        let mut a = sample_entry();
        a.data.message.id = None;
        a.model = Some("model-a".to_string());
        a.data.message.model = Some("model-a".to_string());
        a.cost = 0.01;

        let mut b = sample_entry();
        b.data.message.id = None;
        b.model = Some("model-b".to_string());
        b.data.message.model = Some("model-b".to_string());
        b.cost = 0.02;

        let cold = load_with_cache(
            "ns-synth",
            std::slice::from_ref(&src),
            false,
            0,
            false,
            |_| Ok(vec![a.clone(), b.clone()]),
        )
        .unwrap();
        assert_eq!(cold.len(), 2);

        // Source removed — both must re-emit from the ledger, not collapse.
        fs::remove_file(&src).unwrap();
        let warm = load_with_cache("ns-synth", &[], false, 0, false, |_| Ok(Vec::new())).unwrap();

        assert_eq!(
            warm.len(),
            2,
            "distinct-model entries must not share a synthetic ledger key"
        );
        let total: f64 = warm.iter().map(|e| e.cost).sum();
        assert!(
            (total - 0.03).abs() < 1e-9,
            "both entries' spend must survive deletion"
        );
    }

    /// Two synthetic-keyed entries that share every field except project must
    /// not collapse in the ledger — pi emits entries in multiple projects under
    /// one namespace, and its live dedup key is project-scoped.
    #[test]
    fn synthetic_key_separates_distinct_projects() {
        let _env = CacheEnv::new("synth-distinct-projects");
        let src = write_source("synth-distinct-projects", "line\n");

        let mut a = sample_entry();
        a.data.message.id = None;
        a.project = Arc::from("project-a");
        a.cost = 0.01;

        let mut b = sample_entry();
        b.data.message.id = None;
        b.project = Arc::from("project-b");
        b.cost = 0.02;

        let cold = load_with_cache(
            "ns-proj",
            std::slice::from_ref(&src),
            false,
            0,
            false,
            |_| Ok(vec![a.clone(), b.clone()]),
        )
        .unwrap();
        assert_eq!(cold.len(), 2);

        fs::remove_file(&src).unwrap();
        let warm = load_with_cache("ns-proj", &[], false, 0, false, |_| Ok(Vec::new())).unwrap();

        assert_eq!(
            warm.len(),
            2,
            "distinct-project entries must not share a synthetic ledger key"
        );
        let total: f64 = warm.iter().map(|e| e.cost).sum();
        assert!(
            (total - 0.03).abs() < 1e-9,
            "both entries' spend must survive"
        );
    }

    /// Property: the synthetic ledger key is injective across every billable
    /// dimension it encodes. Mutating any single dimension in isolation must
    /// change the key, and no two single-field mutations may collide. This pins
    /// the full field set — if a future edit drops a dimension from
    /// [`entry_ledger_key`], the row for that dimension fails here, rather than
    /// silently collapsing two distinct calls' spend in the ledger.
    #[test]
    fn synthetic_key_is_injective_per_billable_dimension() {
        type Mutation = (&'static str, fn(&mut LoadedEntry));
        let mutations: &[Mutation] = &[
            ("timestamp", |e| {
                e.timestamp = TimestampMs::from_millis(2_000)
            }),
            ("project", |e| e.project = Arc::from("other-project")),
            ("session_id", |e| e.session_id = Arc::from("other-session")),
            ("model", |e| e.model = Some("other-model".to_string())),
            ("input_tokens", |e| e.data.message.usage.input_tokens = 7),
            ("output_tokens", |e| e.data.message.usage.output_tokens = 7),
            ("cache_creation", |e| {
                e.data.message.usage.cache_creation_input_tokens = 7
            }),
            ("cache_read", |e| {
                e.data.message.usage.cache_read_input_tokens = 7
            }),
            ("extra_total_tokens", |e| e.extra_total_tokens = 7),
        ];

        let mut base = sample_entry();
        base.data.message.id = None; // force the synthetic path
        let base_key = entry_ledger_key(&base);

        let mut seen = HashSet::new();
        seen.insert(base_key.clone());
        for (dimension, mutate) in mutations {
            let mut variant = base.clone();
            mutate(&mut variant);
            let key = entry_ledger_key(&variant);
            assert_ne!(
                key, base_key,
                "mutating {dimension} must change the synthetic ledger key"
            );
            assert!(
                seen.insert(key),
                "{dimension} produced a key already seen — two dimensions collide"
            );
        }
    }

    /// Resume / no double-count: file deleted (spend retained in the ledger),
    /// then recreated and re-parsed; the next `load_with_cache` returns the live
    /// entry exactly once — the live copy wins over the ledger record sharing its
    /// dedup key.
    #[test]
    fn resume_no_double_count() {
        let _env = CacheEnv::new("resume-evict");
        let src = write_source("resume-evict", "original\n");

        let _ = load_with_cache("ns", std::slice::from_ref(&src), false, 0, false, |_| {
            Ok(vec![sample_entry()])
        });

        // Delete: spend is retained in the ledger.
        fs::remove_file(&src).unwrap();
        let _ = load_with_cache("ns", &[], false, 0, false, |_| Ok(Vec::new()));

        fs::write(&src, "new content\n").unwrap();
        let mut new_entry = sample_entry();
        new_entry.session_id = Arc::from("session-new");

        // Live run: exactly 1 entry (live), not 2 (live + ledger).
        let live = load_with_cache("ns", std::slice::from_ref(&src), false, 0, false, |_| {
            Ok(vec![new_entry.clone()])
        })
        .unwrap();
        assert_eq!(live.len(), 1, "must not double-count live + ledger");
        assert_eq!(live[0].session_id.as_ref(), "session-new");

        let _ = fs::remove_file(&src);
    }

    /// Deleted-source tracking is lossless through the slim ledger: an entry
    /// carrying dedup-only identity (`request_id`/`is_sidechain`) is re-emitted
    /// after its source file is gone with every *tracking* field intact, even
    /// though those identity fields are no longer persisted.
    #[test]
    fn deleted_source_tracking_survives_without_identity() {
        let _env = CacheEnv::new("slim-ledger");
        let src = write_source("slim-ledger", "original\n");

        let mut entry = sample_entry();
        entry.cost = 1.25;
        entry.extra_total_tokens = 7;
        entry.credits = Some(3.0);
        entry.message_count = Some(2);
        entry.model = Some("claude-track".to_string());
        entry.data.message.usage = TokenUsageRaw {
            input_tokens: 11,
            output_tokens: 22,
            cache_creation_input_tokens: 5,
            cache_read_input_tokens: 9,
            speed: None,
            cache_creation: None,
        };
        // Dedup-only identity that the slim ledger intentionally drops.
        entry.data.request_id = Some("req-track".to_string());
        entry.data.is_sidechain = Some(true);

        // Cold run caches it; deletion retains its spend in the ledger only.
        let _ = load_with_cache("ns", std::slice::from_ref(&src), false, 0, false, |_| {
            Ok(vec![entry.clone()])
        });
        fs::remove_file(&src).unwrap();

        let warm = load_with_cache("ns", &[], false, 0, false, |_| Ok(Vec::new())).unwrap();

        assert_eq!(
            warm.len(),
            1,
            "deleted spend must be re-emitted exactly once"
        );
        let got = &warm[0];
        assert!((got.cost - 1.25).abs() < 1e-9, "cost must survive");
        assert_eq!(got.extra_total_tokens, 7);
        assert_eq!(got.credits, Some(3.0));
        assert_eq!(got.message_count, Some(2));
        assert_eq!(got.model.as_deref(), Some("claude-track"));
        assert_eq!(got.data.message.usage.input_tokens, 11);
        assert_eq!(got.data.message.usage.output_tokens, 22);
        assert_eq!(got.data.message.usage.cache_creation_input_tokens, 5);
        assert_eq!(got.data.message.usage.cache_read_input_tokens, 9);
        // Identity restored from the key column; dedup-only fields are gone.
        assert_eq!(got.data.message.id.as_deref(), Some("msg-a"));
        assert_eq!(got.data.request_id, None);
        assert_eq!(got.data.is_sidechain, None);

        let _ = fs::remove_file(&src);
    }

    /// Two-phase ledger read: a warm run with a deleted source re-emits its
    /// spend, while a warm run with ALL sources present returns only the live
    /// copies (zero blob reads from the ledger).
    #[test]
    fn two_phase_ledger_deleted_source_reemitted_all_present_unchanged() {
        let _env = CacheEnv::new("two-phase-ledger");
        let src = write_source("two-phase-ledger", "line\n");

        let mut entry_a = sample_entry();
        entry_a.cost = 0.42;
        entry_a.model = Some("model-a".to_string());
        let cold = load_with_cache(
            "ns-two-phase",
            std::slice::from_ref(&src),
            false,
            0,
            false,
            |_| Ok(vec![entry_a.clone()]),
        )
        .unwrap();
        assert_eq!(cold.len(), 1);

        // Delete source -> spend retained in ledger only.
        fs::remove_file(&src).unwrap();
        let warm_deleted =
            load_with_cache("ns-two-phase", &[], false, 0, false, |_| Ok(Vec::new())).unwrap();
        assert_eq!(
            warm_deleted.len(),
            1,
            "deleted source must re-emit from ledger"
        );
        assert!((warm_deleted[0].cost - 0.42).abs() < 1e-9);

        let src2 = write_source("two-phase-ledger", "line2\n");
        let mut entry_b = sample_entry();
        entry_b.session_id = Arc::from("session-b");
        entry_b.cost = 0.99;
        entry_b.model = Some("model-b".to_string());
        // entry_a is still in the ledger; live run provides both A and B.
        let warm_present = load_with_cache(
            "ns-two-phase",
            std::slice::from_ref(&src2),
            false,
            0,
            false,
            |_| Ok(vec![entry_a.clone(), entry_b.clone()]),
        )
        .unwrap();
        assert_eq!(
            warm_present.len(),
            2,
            "all sources present: must return live copies only, no ledger duplicates"
        );
        let total: f64 = warm_present.iter().map(|e| e.cost).sum();
        assert!(
            (total - 1.41).abs() < 1e-9,
            "live entries must win over ledger records"
        );

        let _ = fs::remove_file(&src2);
    }

    /// Namespace isolation: a ledger entry recorded under namespace "a" is NOT
    /// emitted by a `load_with_cache` call for namespace "b".
    #[test]
    fn namespace_isolation_ledger_entries_are_scoped() {
        let _env = CacheEnv::new("ns-isolation");

        // Create file under namespace "a" and cache it.
        let src_a = write_source("ns-isolation-a", "a content\n");
        let _ = load_with_cache(
            "ns-a",
            std::slice::from_ref(&src_a),
            false,
            0,
            false,
            |_| Ok(vec![sample_entry()]),
        );

        // Delete the "a" file so its spend lives only in the ledger under "ns-a".
        fs::remove_file(&src_a).unwrap();
        let _ = load_with_cache("ns-a", &[], false, 0, false, |_| Ok(Vec::new()));

        // Now run namespace "b" with no files — must NOT emit "a"'s ledger entries.
        let result_b = load_with_cache("ns-b", &[], false, 0, false, |_| Ok(Vec::new())).unwrap();
        assert_eq!(result_b.len(), 0, "ns-b must not see ns-a's ledger entries");

        // Create a separate "b" file that still exists on disk, then run "ns-b".
        // The "a" ledger entry stays scoped to "ns-a" and never leaks into "ns-b".
        let src_b = write_source("ns-isolation-b", "b content\n");
        let result_b2 = load_with_cache(
            "ns-b",
            std::slice::from_ref(&src_b),
            false,
            0,
            false,
            |_| Ok(vec![sample_entry()]),
        )
        .unwrap();
        // ns-b gets its own live entry, still not the ns-a ledger entry.
        assert_eq!(result_b2.len(), 1);
        assert_eq!(result_b2[0].session_id.as_ref(), "session-a"); // from sample_entry

        let _ = fs::remove_file(&src_b);
    }

    /// A duplicate ledger record (e.g. from a concurrent double-append) must
    /// collapse to a single emitted entry. The `(namespace, dedup_key)` primary
    /// key makes the second write a no-op, so spend is never double-counted.
    #[test]
    fn duplicate_ledger_records_counted_once() {
        let _env = CacheEnv::new("ledger-dup");
        let src = write_source("ledger-dup", "line\n");

        let _ = load_with_cache(
            "dup-ns",
            std::slice::from_ref(&src),
            false,
            0,
            false,
            |_| Ok(vec![sample_entry()]),
        );

        // Attempt to insert the same key again, simulating a concurrent append.
        // The primary key collapses it to a no-op.
        insert_ledger_row("dup-ns", "msg-a", &sample_entry());

        // Delete the source: the key must emit exactly once.
        fs::remove_file(&src).unwrap();
        let warm = load_with_cache("dup-ns", &[], false, 0, false, |_| Ok(Vec::new())).unwrap();
        assert_eq!(
            warm.len(),
            1,
            "duplicate ledger records must collapse to one"
        );
        assert_eq!(warm[0].session_id.as_ref(), "session-a");
    }

    #[test]
    fn fingerprint_mismatch_forces_reparse() {
        let _env = CacheEnv::new("fp-mismatch");
        let src = write_source("fp-mismatch", "line\n");

        // Cache with fingerprint 1.
        let calls_a = std::sync::atomic::AtomicUsize::new(0);
        let cold = load_with_cache(
            "test",
            std::slice::from_ref(&src),
            false,
            1,
            false,
            |_path| {
                calls_a.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(vec![sample_entry()])
            },
        )
        .unwrap();
        assert_eq!(cold.len(), 1);
        assert_eq!(calls_a.load(std::sync::atomic::Ordering::SeqCst), 1);

        // Load with fingerprint 2 — must reparse, not serve stale cache.
        let calls_b = std::sync::atomic::AtomicUsize::new(0);
        let reparsed = load_with_cache(
            "test",
            std::slice::from_ref(&src),
            false,
            2,
            false,
            |_path| {
                calls_b.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(vec![sample_entry()])
            },
        )
        .unwrap();
        assert_eq!(reparsed.len(), 1);
        assert_eq!(
            calls_b.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "parse_file must run again when fingerprint changes"
        );

        let _ = fs::remove_file(&src);
    }

    #[test]
    fn fingerprint_match_serves_from_cache() {
        let _env = CacheEnv::new("fp-match");
        let src = write_source("fp-match", "line\n");

        // Cache with fingerprint 42.
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let cold = load_with_cache(
            "test",
            std::slice::from_ref(&src),
            false,
            42,
            false,
            |_path| {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(vec![sample_entry()])
            },
        )
        .unwrap();
        assert_eq!(cold.len(), 1);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);

        // Load with same fingerprint 42 — must be a cache hit.
        let warm = load_with_cache(
            "test",
            std::slice::from_ref(&src),
            false,
            42,
            false,
            |_path| {
                panic!("parse_file should not run when fingerprint matches");
            },
        )
        .unwrap();
        assert_eq!(warm.len(), 1);
        assert_eq!(warm[0].session_id.as_ref(), "session-a");

        let _ = fs::remove_file(&src);
    }

    /// Concurrent write-backs sharing the cache database must each preserve the
    /// others' `files` rows. Every thread caches a file under its own namespace;
    /// afterward every row must be present. Each round resets the database to a
    /// fresh, non-WAL file so the openers re-race the `journal_mode=WAL` switch —
    /// the lock upgrade where they used to deadlock and silently drop a
    /// connection along with its row. `set_wal_mode`'s bounded retry plus
    /// `busy_timeout` now serialize the writers so no update is lost.
    #[test]
    fn concurrent_writers_preserve_all_file_rows() {
        let _env = CacheEnv::new("concurrent-files");
        const N: usize = 8;
        // The original single-shot version passed or failed on luck. Re-racing the
        // cold WAL switch over many rounds turns an intermittent drop into a
        // near-certain failure, so a regression cannot sneak through green CI.
        const ROUNDS: usize = 40;
        let srcs: Vec<PathBuf> = (0..N)
            .map(|i| write_source(&format!("concurrent-{i}"), "line\n"))
            .collect();

        for round in 0..ROUNDS {
            // Drop the db and its WAL sidecars so the next opens start from a
            // non-WAL file and contend on the journal_mode switch again.
            clear_cache();
            // Release all threads into the write-back together to maximize contention.
            let barrier = Arc::new(std::sync::Barrier::new(N));
            std::thread::scope(|scope| {
                for (i, src) in srcs.iter().enumerate() {
                    let barrier = Arc::clone(&barrier);
                    scope.spawn(move || {
                        let namespace = format!("ns-{i}");
                        barrier.wait();
                        // single_thread = true so each worker does not spawn its
                        // own parse pool; the one file per thread is parsed inline.
                        let _ = load_with_cache(
                            &namespace,
                            std::slice::from_ref(src),
                            true,
                            0,
                            false,
                            |_| Ok(vec![sample_entry()]),
                        );
                    });
                }
            });

            for (i, src) in srcs.iter().enumerate() {
                let key = src.to_string_lossy().to_string();
                assert!(
                    file_paths(&format!("ns-{i}")).contains(&key),
                    "round {round}: files table lost the row for {key} under concurrent writes"
                );
            }
        }

        for src in &srcs {
            let _ = fs::remove_file(src);
        }
    }

    #[test]
    fn live_only_excludes_retained_entries() {
        let _env = CacheEnv::new("live-only-exclude");
        let src = write_source("live-only-exclude", "line\n");

        let entry = sample_entry();
        let _ = load_with_cache(
            "test-live",
            std::slice::from_ref(&src),
            false,
            0,
            false,
            |_| Ok(vec![entry.clone()]),
        )
        .unwrap();

        // Delete source -> spend retained in ledger only.
        fs::remove_file(&src).unwrap();

        // Default run: re-emits the retained entry.
        let default_result =
            load_with_cache("test-live", &[], false, 0, false, |_| Ok(Vec::new())).unwrap();
        assert_eq!(
            default_result.len(),
            1,
            "default must include retained entries"
        );

        // live_only run: excludes the retained entry.
        let live_only_result =
            load_with_cache("test-live", &[], false, 0, true, |_| Ok(Vec::new())).unwrap();
        assert_eq!(
            live_only_result.len(),
            0,
            "live_only must exclude retained entries"
        );
    }

    #[test]
    fn live_only_still_appends_to_ledger() {
        let _env = CacheEnv::new("live-only-append");
        let src = write_source("live-only-append", "line\n");

        // Run with live_only=true: entries should still be appended to the ledger.
        let entry = sample_entry();
        let _ = load_with_cache(
            "test-live",
            std::slice::from_ref(&src),
            false,
            0,
            true,
            |_| Ok(vec![entry.clone()]),
        )
        .unwrap();

        // Delete source and run with live_only=false: should re-emit from ledger.
        fs::remove_file(&src).unwrap();
        let result =
            load_with_cache("test-live", &[], false, 0, false, |_| Ok(Vec::new())).unwrap();
        assert_eq!(
            result.len(),
            1,
            "ledger must still contain entries appended under live_only"
        );
    }

    #[test]
    fn pricing_store_load_round_trip() {
        let _env = CacheEnv::new("pricing-roundtrip");
        let url = "https://example.com/pricing.json";
        let body = r#"{"models":{}}"#;
        let etag = Some(r#""abc123""#);
        let last_modified = Some("Wed, 09 Apr 2025 12:00:00 GMT");

        store_pricing(url, body, etag, last_modified);
        let cached = load_pricing(url).expect("cache miss after store");

        assert_eq!(cached.body, body);
        assert_eq!(cached.etag.as_deref(), etag);
        assert_eq!(cached.last_modified.as_deref(), last_modified);
    }

    #[test]
    fn pricing_load_returns_none_for_unknown_url() {
        let _env = CacheEnv::new("pricing-miss");
        assert!(load_pricing("https://no-such-url.example.com").is_none());
    }

    #[test]
    fn pricing_upsert_replaces_existing_row() {
        let _env = CacheEnv::new("pricing-upsert");
        let url = "https://example.com/pricing.json";

        store_pricing(url, "v1", Some("old-etag"), Some("old-date"));
        store_pricing(url, "v2", Some("new-etag"), Some("new-date"));

        let cached = load_pricing(url).expect("cache miss after upsert");
        assert_eq!(cached.body, "v2");
        assert_eq!(cached.etag.as_deref(), Some("new-etag"));
        assert_eq!(cached.last_modified.as_deref(), Some("new-date"));
    }

    #[test]
    fn pricing_none_etag_and_last_modified_round_trip() {
        let _env = CacheEnv::new("pricing-none-headers");
        let url = "https://example.com/pricing.json";

        store_pricing(url, "body", None, None);
        let cached = load_pricing(url).expect("cache miss after store");

        assert_eq!(cached.body, "body");
        assert!(cached.etag.is_none());
        assert!(cached.last_modified.is_none());
    }

    /// `migrate()` must create `schema_meta` with exactly four rows at version 1.
    #[test]
    fn migrate_populates_schema_meta_with_four_rows_at_v1() {
        let _env = CacheEnv::new("schema-meta");
        let conn = open_db().expect("open_db should succeed");
        let mut st = conn
            .prepare("SELECT name, version FROM schema_meta ORDER BY name")
            .unwrap();
        let mut rows: Vec<(String, i64)> = Vec::new();
        while let Ok(sqlite::State::Row) = st.next() {
            rows.push((
                st.read::<String, _>(0).unwrap(),
                st.read::<i64, _>(1).unwrap(),
            ));
        }
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0], ("files".to_string(), 1));
        assert_eq!(rows[1], ("ledger".to_string(), 1));
        assert_eq!(rows[2], ("opencode".to_string(), 1));
        assert_eq!(rows[3], ("pricing".to_string(), 1));
    }

    /// A second `open_db()` on the same database must be idempotent — no
    /// duplicate rows or errors.
    #[test]
    fn migrate_is_idempotent_on_second_open_db() {
        let _env = CacheEnv::new("schema-meta-idempotent");
        let _ = open_db().expect("first open_db should succeed");
        let conn = open_db().expect("second open_db should succeed");
        let mut st = conn.prepare("SELECT COUNT(*) FROM schema_meta").unwrap();
        st.next().unwrap();
        let count = st.read::<i64, _>(0).unwrap();
        assert_eq!(count, 4, "idempotent open must not duplicate rows");
    }

    /// `clear_cache()` must remove `cache.db` (and sidecars if present).
    #[test]
    fn clear_cache_removes_cache_db() {
        let _env = CacheEnv::new("clear-cache");
        {
            let _conn = open_db().expect("open_db should succeed");
            let dir = cache_dir().unwrap();
            assert!(
                dir.join(DB_FILE).exists(),
                "cache.db should exist after open"
            );
        }
        clear_cache();
        let dir = cache_dir().unwrap();
        assert!(
            !dir.join(DB_FILE).exists(),
            "cache.db must be removed after clear_cache"
        );
        assert!(
            !dir.join("cache.db-wal").exists(),
            "cache.db-wal must be removed"
        );
        assert!(
            !dir.join("cache.db-shm").exists(),
            "cache.db-shm must be removed"
        );
    }

    /// `clear_cache_namespaces` must delete only the target namespace rows
    /// while preserving rows in other namespaces.
    #[test]
    fn clear_cache_namespaces_preserves_other_namespaces() {
        let _env = CacheEnv::new("clear-ns");
        let src_a = write_source("clear-ns-a", "a\n");
        let src_b = write_source("clear-ns-b", "b\n");

        let _ = load_with_cache(
            "claude",
            std::slice::from_ref(&src_a),
            false,
            0,
            false,
            |_| Ok(vec![sample_entry()]),
        );
        let _ = load_with_cache(
            "opencode",
            std::slice::from_ref(&src_b),
            false,
            0,
            false,
            |_| Ok(vec![sample_entry()]),
        );

        clear_cache_namespaces("claude");

        let conn = open_db().expect("open after clear");
        let mut st = conn
            .prepare("SELECT COUNT(*) FROM files WHERE namespace = 'claude'")
            .unwrap();
        st.next().unwrap();
        let count_claude = st.read::<i64, _>(0).unwrap();
        assert_eq!(count_claude, 0, "claude files must be deleted");

        let mut st = conn
            .prepare("SELECT COUNT(*) FROM files WHERE namespace = 'opencode'")
            .unwrap();
        st.next().unwrap();
        let count_opencode = st.read::<i64, _>(0).unwrap();
        assert_eq!(count_opencode, 1, "opencode files must be preserved");

        let _ = fs::remove_file(&src_a);
        let _ = fs::remove_file(&src_b);
    }
}
