use std::{
    fs,
    io::{self, Read as _},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use ccusage_core::pricing::FetchedJson;

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
/// bytes. The cache only ever answers a 304 — any fetch failure surfaces as an
/// error so the caller's embedded pricing, which every release refreshes, takes
/// over exactly as it does with no cache at all. It is best-effort like the
/// statusline cache: a missing or torn entry just means a full fetch.
pub(crate) fn fetch_json(url: &str) -> io::Result<FetchedJson> {
    fetch_json_with_cache_dir(url, default_cache_dir().as_deref())
}

/// Per-user cache directory: `XDG_CACHE_HOME`, or `.cache` under the home
/// directory (`ccusage_core::home::home_dir`, which also resolves the Windows
/// profile variables). `None` disables caching for the run: without a per-user
/// directory the only candidates are world-shared places like `/tmp`, where
/// another local user could plant or symlink an entry that a later 304 would
/// turn into trusted pricing data.
fn default_cache_dir() -> Option<PathBuf> {
    cache_dir_under(
        std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from),
        ccusage_core::home::home_dir(),
    )
}

fn cache_dir_under(xdg_cache_home: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    xdg_cache_home
        .filter(|path| path.is_absolute())
        .or_else(|| {
            home.filter(|path| path.is_absolute())
                .map(|home| home.join(".cache"))
        })
        .map(|base| base.join("ccusage").join("http-cache"))
}

fn fetch_json_with_cache_dir(url: &str, cache_dir: Option<&Path>) -> io::Result<FetchedJson> {
    let cache = cache_dir.map(|dir| CacheEntry::for_url(dir, url));
    let cached = cache.as_ref().and_then(CacheEntry::read);

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
        Err(ureq::Error::StatusCode(code)) => {
            return Err(io::Error::other(format!("HTTP {code}")));
        }
        Err(error) => return Err(io::Error::other(error.to_string())),
    };
    let status = response.status().as_u16();
    if status == 304 {
        return match cached {
            Some(cached) => Ok(FetchedJson::new(cached.body)),
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
    response
        .body_mut()
        .with_config()
        .reader()
        .take(PRICING_FETCH_MAX_BYTES + 1)
        .read_to_string(&mut body)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    if body.len() as u64 > PRICING_FETCH_MAX_BYTES {
        return Err(io::Error::other(format!(
            "response body over {PRICING_FETCH_MAX_BYTES} bytes"
        )));
    }
    // The cache write is deferred to the caller's parse verdict: only a body
    // the pricing loader accepted may replace the previous validated pair, so
    // a hijacked or errored 200 (captive portal, CDN error page) never evicts
    // it even though the body itself is still handed to the caller.
    match (cache, etag) {
        (Some(cache), Some(etag)) => Ok(FetchedJson::with_validation_hook(
            body,
            Box::new(move |body| cache.write(&etag, body)),
        )),
        _ => Ok(FetchedJson::new(body)),
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
        self.read_bounded(PRICING_FETCH_MAX_BYTES)
    }

    /// Bounded like the network read: the cache file holds a past response
    /// body, so a grown or corrupted file must not balloon memory either.
    fn read_bounded(&self, max_bytes: u64) -> Option<CachedResponse> {
        let file = fs::File::open(&self.path).ok()?;
        let mut raw = String::new();
        file.take(max_bytes.saturating_add(1))
            .read_to_string(&mut raw)
            .ok()?;
        if raw.len() as u64 > max_bytes {
            return None;
        }
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
    use super::{CacheEntry, cache_dir_under, cache_file_stem, fetch_json_with_cache_dir};
    use ccusage_test_support::Fixture;
    use std::{
        io::{Read as _, Write as _},
        net::TcpListener,
        path::PathBuf,
        thread,
    };

    /// Minimal scripted HTTP server: serves one connection per response and
    /// returns the request head each connection sent.
    fn serve_responses(responses: Vec<String>) -> (String, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let url = format!(
            "http://{}/pricing.json",
            listener.local_addr().expect("mock server address")
        );
        let handle = thread::spawn(move || {
            let mut requests = Vec::new();
            for response in responses {
                let (mut stream, _) = listener.accept().expect("accept connection");
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if stream.read(&mut byte).expect("read request head") == 0 {
                        break;
                    }
                    head.push(byte[0]);
                }
                requests.push(String::from_utf8_lossy(&head).into_owned());
                stream
                    .write_all(response.as_bytes())
                    .expect("write response");
            }
            requests
        });
        (url, handle)
    }

    fn ok_response(etag: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\netag: {etag}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len(),
        )
    }

    #[test]
    fn fetch_writes_cache_only_on_commit_and_then_revalidates() {
        let fixture = Fixture::new();
        let cache_dir = fixture.path("http-cache");
        let body = "{\"model\":{}}";
        let (url, server) = serve_responses(vec![
            ok_response("\"v1\"", body),
            ok_response("\"v1\"", body),
            "HTTP/1.1 304 Not Modified\r\netag: \"v1\"\r\nconnection: close\r\n\r\n".to_string(),
        ]);
        let entry = CacheEntry::for_url(&cache_dir, &url);

        // An uncommitted body must leave no cache entry behind.
        let uncommitted = fetch_json_with_cache_dir(&url, Some(&cache_dir)).expect("first fetch");
        assert_eq!(uncommitted.body(), body);
        drop(uncommitted);
        assert!(
            entry.read().is_none(),
            "a body the caller never validated must not be cached"
        );

        // Committing after the caller's parse persists the pair.
        let fetched = fetch_json_with_cache_dir(&url, Some(&cache_dir)).expect("second fetch");
        fetched.commit();
        let committed = entry.read().expect("commit writes the etag/body pair");
        assert_eq!(committed.etag, "\"v1\"");
        assert_eq!(committed.body, body);

        // A cached pair revalidates and a 304 serves the body from disk.
        let revalidated =
            fetch_json_with_cache_dir(&url, Some(&cache_dir)).expect("revalidated fetch");
        assert_eq!(revalidated.body(), body);

        let requests = server.join().expect("mock server");
        assert!(
            !requests[0].to_lowercase().contains("if-none-match"),
            "no validator may be offered before anything is cached"
        );
        assert!(
            !requests[1].to_lowercase().contains("if-none-match"),
            "an uncommitted body must not become a validator"
        );
        assert!(
            requests[2].to_lowercase().contains("if-none-match: \"v1\""),
            "the committed pair must revalidate: {}",
            requests[2]
        );
    }

    #[test]
    fn fetch_failure_is_an_error_even_with_a_cached_copy() {
        let fixture = Fixture::new();
        let cache_dir = fixture.path("http-cache");
        let (url, server) = serve_responses(vec![
            "HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                .to_string(),
        ]);
        let entry = CacheEntry::for_url(&cache_dir, &url);
        entry.write("\"v1\"", "{\"model\":{}}");

        let error = fetch_json_with_cache_dir(&url, Some(&cache_dir))
            .expect_err("a failed refresh must surface so embedded pricing takes over");
        assert_eq!(error.to_string(), "HTTP 500");
        // The pair stays available for revalidation; it is just never a body.
        assert!(entry.read().is_some());
        let _ = server.join();
    }

    #[test]
    fn missing_cache_dir_disables_caching_but_not_fetching() {
        let body = "{\"model\":{}}";
        let (url, server) = serve_responses(vec![ok_response("\"v1\"", body)]);

        let fetched = fetch_json_with_cache_dir(&url, None).expect("fetch without a cache dir");
        assert_eq!(fetched.body(), body);
        // With no per-user directory there is nowhere safe to write: commit
        // must be a no-op instead of a fallback to a shared location.
        fetched.commit();

        let requests = server.join().expect("mock server");
        assert!(!requests[0].to_lowercase().contains("if-none-match"));
    }

    #[test]
    fn cache_round_trips_etag_and_body() {
        let fixture = Fixture::new();
        let entry = CacheEntry::for_url(fixture.root(), "https://example.com/pricing.json");

        assert!(entry.read().is_none());
        entry.write("\"abc123\"", "{\"model\":{}}");
        let cached = entry.read().expect("cache entry after write");
        assert_eq!(cached.etag, "\"abc123\"");
        assert_eq!(cached.body, "{\"model\":{}}");
    }

    #[test]
    fn cache_read_rejects_unusable_etag() {
        let fixture = Fixture::new();
        let entry = CacheEntry::for_url(fixture.root(), "https://example.com/pricing.json");

        entry.write("\"ok\"", "body");
        std::fs::write(&entry.path, "   \nbody").expect("overwrite cache");
        assert!(entry.read().is_none(), "blank etag must not revalidate");
        std::fs::write(&entry.path, "etag\rwith-cr\nbody").expect("overwrite cache");
        assert!(
            entry.read().is_none(),
            "control characters must not reach a header value"
        );
        std::fs::write(&entry.path, "etag-without-newline").expect("overwrite cache");
        assert!(entry.read().is_none(), "a truncated entry must not be used");
    }

    #[test]
    fn cache_read_is_bounded_like_the_network_read() {
        let fixture = Fixture::new();
        let entry = CacheEntry::for_url(fixture.root(), "https://example.com/pricing.json");

        entry.write("\"v1\"", "0123456789");
        assert!(
            entry.read_bounded(4).is_none(),
            "an oversize cache file must be ignored, not loaded"
        );
        assert!(entry.read_bounded(64).is_some());
    }

    #[test]
    fn cache_write_replaces_pair_atomically_and_cleans_up() {
        let fixture = Fixture::new();
        let entry = CacheEntry::for_url(fixture.root(), "https://example.com/pricing.json");

        entry.write("\"v1\"", "first");
        entry.write("\"v2\"", "second\nwith\nnewlines");
        let cached = entry.read().expect("cache entry after rewrite");
        assert_eq!(cached.etag, "\"v2\"");
        assert_eq!(cached.body, "second\nwith\nnewlines");
        // No temp files may linger after successful writes.
        let leftovers: Vec<_> = std::fs::read_dir(fixture.root())
            .expect("cache dir")
            .filter_map(Result::ok)
            .filter(|e| e.path() != entry.path)
            .collect();
        assert!(leftovers.is_empty(), "leftover files: {leftovers:?}");
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
    fn cache_dir_requires_an_absolute_per_user_directory() {
        let fixture = Fixture::new();
        let xdg = fixture.path("xdg-cache");
        let home = fixture.path("home");

        assert_eq!(
            cache_dir_under(Some(xdg.clone()), Some(home.clone())),
            Some(xdg.join("ccusage").join("http-cache")),
        );
        assert_eq!(
            cache_dir_under(None, Some(home.clone())),
            Some(home.join(".cache").join("ccusage").join("http-cache")),
        );
        // A relative XDG_CACHE_HOME must be ignored per the basedir spec.
        assert_eq!(
            cache_dir_under(Some(PathBuf::from("relative")), Some(home.clone())),
            Some(home.join(".cache").join("ccusage").join("http-cache")),
        );
        // No per-user directory disables caching entirely rather than falling
        // back to a world-shared temp dir another local user can write to.
        assert_eq!(cache_dir_under(Some(PathBuf::from("relative")), None), None);
        assert_eq!(cache_dir_under(None, None), None);
    }
}
