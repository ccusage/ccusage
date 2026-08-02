use std::{
    fs,
    io::{self, Read as _},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

const PRICING_FETCH_TIMEOUT_SECONDS: u64 = 10;
const PRICING_FETCH_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Fetches a JSON document for the pricing refresh.
///
/// This lives in the binary so that `ureq` and its TLS stack are not dependencies
/// of `ccusage-core`, which every adapter builds against; `main` installs it
/// through `ccusage_core::pricing::set_json_fetcher`.
///
/// Each response body is kept on disk next to its ETag, and later fetches
/// revalidate with `If-None-Match`: an unchanged document costs a bodyless 304
/// instead of the full download. GitHub serves the pricing table with an ETag
/// (a weak validator on the gzip variant, which `If-None-Match` still matches),
/// so a 1-minute polling loop drops from ~1.7MB per run to a few hundred
/// bytes. The cache is best-effort like the statusline cache: a missing or
/// torn entry just means a full fetch.
pub(crate) fn fetch_json(url: &str) -> io::Result<String> {
    fetch_json_with_cache_dir(url, &default_cache_dir())
}

/// Per-user cache directory. `XDG_CACHE_HOME` (or `~/.cache`) keeps the cache
/// out of any world-shared /tmp on Linux, where another local user could plant
/// or symlink entries; the temp dir — already per-user on macOS and Windows —
/// is only the fallback when no home is available.
fn default_cache_dir() -> PathBuf {
    cache_dir_under(
        std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
        std::env::temp_dir(),
    )
}

fn cache_dir_under(
    xdg_cache_home: Option<PathBuf>,
    home: Option<PathBuf>,
    temp_dir: PathBuf,
) -> PathBuf {
    xdg_cache_home
        .filter(|path| path.is_absolute())
        .or_else(|| {
            home.filter(|path| path.is_absolute())
                .map(|home| home.join(".cache"))
        })
        .unwrap_or(temp_dir)
        .join("ccusage")
        .join("http-cache")
}

fn fetch_json_with_cache_dir(url: &str, cache_dir: &Path) -> io::Result<String> {
    let cache = CacheEntry::for_url(cache_dir, url);
    let cached = cache.read();

    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(PRICING_FETCH_TIMEOUT_SECONDS)))
        .build()
        .new_agent();
    let mut request = agent.get(url);
    if let Some(cached) = &cached {
        request = request.header("if-none-match", &cached.etag);
    }
    let mut response = match request.call() {
        Ok(response) => response,
        // The URL itself no longer works (moved, gone): surface the error so
        // the caller's embedded fallback — which every release refreshes —
        // wins over a cache frozen at the last successful fetch.
        Err(ureq::Error::StatusCode(code)) if is_permanent_http_failure(code) => {
            return Err(io::Error::other(format!("HTTP {code}")));
        }
        // Everything else is treated as transient: a previously validated
        // body on disk beats taking the run down.
        Err(error) => {
            return match cached {
                Some(cached) => {
                    log_stale_cache_use(url, &error.to_string());
                    Ok(cached.body)
                }
                None => Err(io::Error::other(error.to_string())),
            };
        }
    };
    let status = response.status().as_u16();
    if status == 304 {
        return match cached {
            Some(cached) => Ok(cached.body),
            // Only reachable if the server sends an unsolicited 304.
            None => Err(io::Error::other("HTTP 304 without a cached body")),
        };
    }
    if status != 200 {
        return Err(io::Error::other(format!("HTTP {status}")));
    }
    let etag = response
        .headers()
        .get("etag")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    // Bound the DECOMPRESSED size: ureq's own `.limit()` sits below the gzip
    // decoder, so it would only cap the compressed stream and a hostile
    // origin could balloon memory ~1000x through it. `take` on the reader
    // bounds what actually lands in the string.
    let mut body = String::new();
    let failure = match response
        .body_mut()
        .with_config()
        .reader()
        .take(PRICING_FETCH_MAX_BYTES + 1)
        .read_to_string(&mut body)
    {
        Ok(_) if body.len() as u64 > PRICING_FETCH_MAX_BYTES => Some(io::Error::other(format!(
            "response body over {PRICING_FETCH_MAX_BYTES} bytes"
        ))),
        Ok(_) => None,
        Err(error) => Some(io::Error::new(
            io::ErrorKind::InvalidData,
            error.to_string(),
        )),
    };
    // A body that dies mid-read (timeout, truncated stream, oversize) gets
    // the same stale fallback as a failed request.
    if let Some(error) = failure {
        return match cached {
            Some(cached) => {
                log_stale_cache_use(url, &error.to_string());
                Ok(cached.body)
            }
            None => Err(error),
        };
    }
    // Only a body that parses as JSON may replace the cached copy: the stale
    // fallback is only sound while the copy on disk is known-good, and a
    // hijacked or errored 200 (captive portal, CDN error page) must not
    // evict it. The invalid body is still returned — the caller's own parse
    // handling decides what happens to the run itself.
    if let Some(etag) = etag
        && looks_like_json(&body)
    {
        cache.write(&etag, &body);
    }
    Ok(body)
}

/// A 404 or 410 means the URL itself is dead, so serving the stale cache
/// forever would mask it. Every other status — 408, 421, 425, 429, 5xx —
/// can be a passing condition and falls back to the cached copy.
fn is_permanent_http_failure(code: u16) -> bool {
    matches!(code, 404 | 410)
}

fn looks_like_json(body: &str) -> bool {
    serde_json::from_str::<serde::de::IgnoredAny>(body).is_ok()
}

/// Mirrors the WARN gating of the pricing refresh in `ccusage-core`: silent
/// by default, visible at log level 4+ so serving a stale cached copy is
/// never invisible to someone debugging pricing.
fn log_stale_cache_use(url: &str, error: &str) {
    if ccusage_core::log_level().is_some_and(|level| level >= 4) {
        eprintln!("WARN  Failed to refresh {url} ({error}); serving cached copy.");
    }
}

/// On-disk ETag + body pair for one URL, stored as a single file: the first
/// line is the ETag, everything after it is the body. Keeping the pair in one
/// file means one atomic rename replaces both together, so concurrent runs
/// (a statusline and a polling collector, say) never observe a body paired
/// with another run's ETag.
struct CacheEntry {
    path: PathBuf,
}

struct CachedResponse {
    etag: String,
    body: String,
}

impl CacheEntry {
    fn for_url(dir: &Path, url: &str) -> Self {
        Self {
            path: dir.join(format!("{}.cache", cache_file_stem(url))),
        }
    }

    fn read(&self) -> Option<CachedResponse> {
        let raw = fs::read_to_string(&self.path).ok()?;
        let (etag, body) = raw.split_once('\n')?;
        let etag = etag.trim();
        // The ETag goes back out as a header value, so drop anything that
        // could not have come from one.
        if etag.is_empty() || !etag.is_ascii() || etag.bytes().any(|byte| byte.is_ascii_control()) {
            return None;
        }
        Some(CachedResponse {
            etag: etag.to_string(),
            body: body.to_string(),
        })
    }

    fn write(&self, etag: &str, body: &str) {
        // Unique per process AND per call: two threads of one process racing
        // on the same URL must not interleave writes into one temp file.
        static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let Some(file_name) = self.path.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        let tmp_path = self.path.with_file_name(format!(
            "{file_name}.tmp.{}.{}",
            std::process::id(),
            TMP_SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        // Rename within a directory is atomic, so readers see either the old
        // pair or the new pair, never a torn or mixed one.
        match fs::write(&tmp_path, format!("{etag}\n{body}")) {
            Ok(()) => {
                if fs::rename(&tmp_path, &self.path).is_err() {
                    let _ = fs::remove_file(&tmp_path);
                }
            }
            // A half-written temp file (out of disk space, say) must not
            // linger either.
            Err(_) => {
                let _ = fs::remove_file(&tmp_path);
            }
        }
    }
}

/// Derives a filesystem-safe, collision-resistant file stem from a URL.
fn cache_file_stem(url: &str) -> String {
    // FNV-1a keeps distinct URLs apart even after the lossy readable prefix.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in url.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    let name: String = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .take(80)
        .collect();
    format!("{name}-{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::{CacheEntry, cache_dir_under, cache_file_stem};
    use std::{fs, path::PathBuf};

    fn unique_test_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "ccusage-http-cache-test-{label}-{}",
            std::process::id()
        ))
    }

    #[test]
    fn cache_round_trips_etag_and_body() {
        let dir = unique_test_dir("round-trip");
        let _ = fs::remove_dir_all(&dir);
        let entry = CacheEntry::for_url(&dir, "https://example.com/pricing.json");

        assert!(entry.read().is_none());
        entry.write("\"abc123\"", "{\"model\":{}}");
        let cached = entry.read().expect("cache entry after write");
        assert_eq!(cached.etag, "\"abc123\"");
        assert_eq!(cached.body, "{\"model\":{}}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_read_rejects_unusable_etag() {
        let dir = unique_test_dir("bad-etag");
        let _ = fs::remove_dir_all(&dir);
        let entry = CacheEntry::for_url(&dir, "https://example.com/pricing.json");

        entry.write("\"ok\"", "body");
        fs::write(&entry.path, "   \nbody").expect("overwrite cache");
        assert!(entry.read().is_none(), "blank etag must not revalidate");
        fs::write(&entry.path, "etag\rwith-cr\nbody").expect("overwrite cache");
        assert!(
            entry.read().is_none(),
            "control characters must not reach a header value"
        );
        fs::write(&entry.path, "etag-without-newline").expect("overwrite cache");
        assert!(entry.read().is_none(), "a truncated entry must not be used");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_write_replaces_pair_atomically_and_cleans_up() {
        let dir = unique_test_dir("atomic");
        let _ = fs::remove_dir_all(&dir);
        let entry = CacheEntry::for_url(&dir, "https://example.com/pricing.json");

        entry.write("\"v1\"", "first");
        entry.write("\"v2\"", "second\nwith\nnewlines");
        let cached = entry.read().expect("cache entry after rewrite");
        assert_eq!(cached.etag, "\"v2\"");
        assert_eq!(cached.body, "second\nwith\nnewlines");
        // No temp files may linger after successful writes.
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .expect("cache dir")
            .filter_map(Result::ok)
            .filter(|e| e.path() != entry.path)
            .collect();
        assert!(leftovers.is_empty(), "leftover files: {leftovers:?}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_file_stem_is_filesystem_safe_and_distinct_per_url() {
        let litellm = cache_file_stem(
            "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json",
        );
        let models_dev = cache_file_stem("https://models.dev/api.json");

        assert_ne!(litellm, models_dev);
        for stem in [&litellm, &models_dev] {
            assert!(
                stem.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')),
                "stem {stem} must be filesystem-safe"
            );
        }
        // The readable prefix is capped; the hash keeps long URLs distinct.
        assert_ne!(
            cache_file_stem(&format!("https://example.com/{}/a.json", "x".repeat(200))),
            cache_file_stem(&format!("https://example.com/{}/b.json", "x".repeat(200))),
        );
    }

    #[test]
    fn permanent_failure_is_a_short_allowlist() {
        use super::is_permanent_http_failure;
        assert!(is_permanent_http_failure(404));
        assert!(is_permanent_http_failure(410));
        for transient in [400u16, 401, 403, 408, 421, 425, 429, 500, 502, 503] {
            assert!(
                !is_permanent_http_failure(transient),
                "HTTP {transient} must fall back to the cached copy"
            );
        }
    }

    #[test]
    fn only_valid_json_may_replace_the_cached_copy() {
        use super::looks_like_json;
        assert!(looks_like_json("{\"model\":{}}"));
        assert!(looks_like_json("[1, 2, 3]"));
        assert!(!looks_like_json("<html>captive portal</html>"));
        assert!(!looks_like_json("{\"truncated\":"));
        assert!(!looks_like_json(""));
    }

    #[test]
    fn cache_dir_prefers_xdg_then_home_then_temp() {
        assert_eq!(
            cache_dir_under(
                Some(PathBuf::from("/xdg")),
                Some(PathBuf::from("/home/u")),
                PathBuf::from("/tmp"),
            ),
            PathBuf::from("/xdg/ccusage/http-cache"),
        );
        assert_eq!(
            cache_dir_under(None, Some(PathBuf::from("/home/u")), PathBuf::from("/tmp")),
            PathBuf::from("/home/u/.cache/ccusage/http-cache"),
        );
        // A relative XDG_CACHE_HOME must be ignored per the basedir spec.
        assert_eq!(
            cache_dir_under(Some(PathBuf::from("relative")), None, PathBuf::from("/tmp")),
            PathBuf::from("/tmp/ccusage/http-cache"),
        );
    }
}
