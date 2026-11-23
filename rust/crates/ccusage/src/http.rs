use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use ccusage_cli::environment::Environment;

mod tls;

const PRICING_FETCH_TIMEOUT_SECONDS: u64 = 10;
const PRICING_FETCH_MAX_BYTES: u64 = 64 * 1024 * 1024;
const CACHE_ETAG_MAX_BYTES: u64 = 4096;
const CACHE_FILE_MAX_BYTES: u64 = PRICING_FETCH_MAX_BYTES + CACHE_ETAG_MAX_BYTES + 1;

pub(crate) fn fetch_json(url: &str, load: &mut dyn FnMut(&str) -> bool) -> io::Result<()> {
    let cache_dir = default_cache_dir();
    fetch_json_with_cache_dir(url, cache_dir.as_deref(), load)
}

pub(crate) fn post_json(url: &str, body: &str, timeout: Duration) -> io::Result<()> {
    let status = http_agent(timeout)
        .post(url)
        .header("content-type", "application/json")
        .send(body)
        .map_err(|error| io::Error::other(error.to_string()))?
        .status()
        .as_u16();
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(io::Error::other(format!("HTTP {status}")))
    }
}

fn default_cache_dir() -> Option<PathBuf> {
    cache_dir_under(
        Environment::current()
            .var_os("XDG_CACHE_HOME")
            .map(PathBuf::from),
        ccusage_core::cli::home::home_dir(),
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

fn fetch_json_with_cache_dir(
    url: &str,
    cache_dir: Option<&Path>,
    load: &mut dyn FnMut(&str) -> bool,
) -> io::Result<()> {
    let cache = cache_dir.map(|dir| CacheEntry::for_url(dir, url));
    let cached = cache.as_ref().and_then(CacheEntry::read);

    let agent = http_agent(Duration::from_secs(PRICING_FETCH_TIMEOUT_SECONDS));
    let mut request = agent.get(url);
    if let Some(cached) = cached.as_ref() {
        request = request.header("if-none-match", &cached.etag);
    }
    let mut response = request
        .call()
        .map_err(|error| io::Error::other(error.to_string()))?;
    let status = response.status().as_u16();

    if status == 304 {
        let Some(cached) = cached else {
            return Err(io::Error::other("HTTP 304 without a cached body"));
        };
        if !load(&cached.body)
            && let Some(cache) = cache.as_ref()
        {
            cache.invalidate();
        }
        return Ok(());
    }
    if status != 200 {
        return Err(io::Error::other(format!("HTTP {status}")));
    }

    if response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length > PRICING_FETCH_MAX_BYTES)
    {
        return Err(io::Error::other(format!(
            "response body over {PRICING_FETCH_MAX_BYTES} bytes"
        )));
    }

    let body = read_bounded_body(
        response.body_mut().with_config().reader(),
        PRICING_FETCH_MAX_BYTES,
    )
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    if load(&body)
        && let Some(etag) = response
            .headers()
            .get("etag")
            .and_then(|value| value.to_str().ok())
        && let Some(cache) = cache.as_ref()
    {
        cache.write(etag, &body);
    }
    Ok(())
}

fn http_agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .unversioned_rustls_crypto_provider(Arc::new(tls::provider()))
                .build(),
        )
        .build()
        .new_agent()
}

fn read_bounded_body(mut reader: impl Read, max_bytes: u64) -> io::Result<String> {
    let mut body = String::new();
    let bytes_read = reader
        .by_ref()
        .take(max_bytes + 1)
        .read_to_string(&mut body)?;
    if bytes_read as u64 > max_bytes {
        return Err(io::Error::other(format!(
            "response body over {max_bytes} bytes"
        )));
    }
    Ok(body)
}

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
        let mut raw = String::new();
        let bytes_read = fs::File::open(&self.path)
            .ok()?
            .take(CACHE_FILE_MAX_BYTES + 1)
            .read_to_string(&mut raw)
            .ok()?;
        if bytes_read as u64 > CACHE_FILE_MAX_BYTES {
            return None;
        }
        let (etag, body) = raw.split_once('\n')?;
        if body.len() as u64 > PRICING_FETCH_MAX_BYTES {
            return None;
        }
        let etag = etag.trim();
        if !valid_etag(etag) {
            return None;
        }
        Some(CachedResponse {
            etag: etag.to_string(),
            body: body.to_string(),
        })
    }

    fn write(&self, etag: &str, body: &str) {
        if !valid_etag(etag) {
            return;
        }
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
        match fs::write(&tmp_path, format!("{etag}\n{body}")) {
            Ok(()) => {
                if fs::rename(&tmp_path, &self.path).is_err() {
                    let _ = fs::remove_file(&tmp_path);
                }
            }
            Err(_) => {
                let _ = fs::remove_file(&tmp_path);
            }
        }
    }

    fn invalidate(&self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn valid_etag(etag: &str) -> bool {
    !etag.is_empty()
        && etag.len() as u64 <= CACHE_ETAG_MAX_BYTES
        && etag.is_ascii()
        && !etag.bytes().any(|byte| byte.is_ascii_control())
}

fn cache_file_stem(url: &str) -> String {
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
    use super::{
        CacheEntry, PRICING_FETCH_MAX_BYTES, cache_dir_under, cache_file_stem,
        fetch_json_with_cache_dir, read_bounded_body,
    };
    use ccusage_test_support::{Fixture, read_fixture_dir};
    use std::{
        fs,
        io::{self, Cursor, Read as _, Write as _},
        net::{TcpListener, TcpStream},
        path::{Path, PathBuf},
        thread,
        time::{Duration, Instant},
    };

    const CACHED_BODY: &str = r#"{"cached":true}"#;
    const FRESH_BODY: &str = r#"{"fresh":true}"#;

    fn fetch(url: &str, dir: &Path, accept: bool) -> (io::Result<()>, Vec<String>) {
        let mut bodies = Vec::new();
        let result = fetch_json_with_cache_dir(url, Some(dir), &mut |body| {
            bodies.push(body.to_string());
            accept
        });
        (result, bodies)
    }

    fn accept_with_timeout(listener: &TcpListener) -> io::Result<TcpStream> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false)?;
                    return Ok(stream);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "timed out waiting for test request",
                        ));
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn read_request(stream: &mut TcpStream) -> io::Result<String> {
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        let mut request = Vec::new();
        let mut byte = [0u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            if stream.read(&mut byte)? == 0 {
                break;
            }
            request.push(byte[0]);
        }
        Ok(String::from_utf8_lossy_owned(request))
    }

    fn serve_responses(
        responses: Vec<String>,
    ) -> (String, thread::JoinHandle<io::Result<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        listener
            .set_nonblocking(true)
            .expect("make test server nonblocking");
        let url = format!(
            "http://{}/pricing.json",
            listener.local_addr().expect("test server address")
        );
        let handle = ccusage_core::parallel::spawn_worker(move || {
            let mut requests = Vec::new();
            for response in responses {
                let mut stream = accept_with_timeout(&listener)?;
                requests.push(read_request(&mut stream)?);
                stream.write_all(response.as_bytes())?;
            }
            Ok(requests)
        });
        (url, handle)
    }

    fn close_after_request() -> (String, thread::JoinHandle<io::Result<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        listener
            .set_nonblocking(true)
            .expect("make test server nonblocking");
        let url = format!(
            "http://{}/pricing.json",
            listener.local_addr().expect("test server address")
        );
        let handle = ccusage_core::parallel::spawn_worker(move || {
            let mut stream = accept_with_timeout(&listener)?;
            read_request(&mut stream)
        });
        (url, handle)
    }

    fn ok_response(etag: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\netag: {etag}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len(),
        )
    }

    fn not_modified_response() -> String {
        "HTTP/1.1 304 Not Modified\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_string()
    }

    fn unique_test_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "ccusage-http-cache-test-{label}-{}",
            std::process::id()
        ))
    }

    #[test]
    fn cache_round_trips_etag_and_body_without_validating_until_revalidation() {
        let dir = unique_test_dir("round-trip");
        let _ = fs::remove_dir_all(&dir);
        let entry = CacheEntry::for_url(&dir, "https://example.com/pricing.json");

        assert!(entry.read().is_none());
        entry.write("\"abc123\"", "{}");
        let cached = entry.read().expect("cache entry after write");
        assert_eq!(cached.etag, "\"abc123\"");
        assert_eq!(cached.body, "{}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_read_rejects_unusable_etag_or_truncated_entry() {
        let dir = unique_test_dir("bad-etag");
        let _ = fs::remove_dir_all(&dir);
        let entry = CacheEntry::for_url(&dir, "https://example.com/pricing.json");

        entry.write("\"ok\"", "{}");
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
    fn cache_read_rejects_body_over_the_pricing_limit_after_the_etag() {
        let dir = unique_test_dir("oversized-body");
        let _ = fs::remove_dir_all(&dir);
        let entry = CacheEntry::for_url(&dir, "https://example.com/pricing.json");

        entry.write("etag", "small body");
        let body = "x".repeat((PRICING_FETCH_MAX_BYTES + 1) as usize);
        fs::write(&entry.path, format!("etag\n{body}")).expect("overwrite cache");

        assert!(
            entry.read().is_none(),
            "the body limit must not include the ETag allowance"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_writes_replace_the_pair_atomically_and_leave_no_temporary_files() {
        let fixture = Fixture::new();
        let dir = fixture.path("http-cache");
        let url = "https://example.com/pricing.json";
        let pairs = Vec::from_fn(8, |index| {
            (
                format!("\"etag-{index}\""),
                format!("body-{index}\nwith-newlines"),
            )
        });
        let handles: Vec<_> = pairs
            .iter()
            .cloned()
            .map(|(etag, body)| {
                let dir = dir.clone();
                let url = url.to_string();
                ccusage_core::parallel::spawn_worker(move || {
                    CacheEntry::for_url(&dir, &url).write(&etag, &body)
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("cache writer");
        }

        let entry = CacheEntry::for_url(&dir, url);
        let cached = entry.read().expect("cache entry after concurrent writes");
        assert!(
            pairs
                .iter()
                .any(|(etag, body)| cached.etag == *etag && cached.body == *body),
            "read a torn cache pair: {:?}",
            (cached.etag, cached.body)
        );
        let leftovers: Vec<_> = read_fixture_dir(&dir)
            .expect("open HTTP cache directory")
            .map(|result| result.expect("read HTTP cache directory entry").path())
            .filter(|path| *path != entry.path)
            .collect();
        assert!(
            leftovers.is_empty(),
            "leftover temporary files: {leftovers:?}"
        );
    }

    #[test]
    fn hands_the_cached_body_to_the_loader_after_304() {
        let fixture = Fixture::new();
        let dir = fixture.path("http-cache");
        let (url, server) = serve_responses(vec![not_modified_response()]);
        let entry = CacheEntry::for_url(&dir, &url);
        entry.write("\"cached\"", CACHED_BODY);

        let (result, bodies) = fetch(&url, &dir, true);
        let requests = server
            .join()
            .expect("test server")
            .expect("server response");

        result.expect("304 should reuse the cached body");
        assert_eq!(bodies, [CACHED_BODY]);
        let request = requests.first().expect("one request").to_ascii_lowercase();
        assert!(request.contains("if-none-match: \"cached\""));
        assert!(request.contains("accept-encoding: gzip"));
        assert_eq!(entry.read().expect("cache kept").body, CACHED_BODY);
    }

    #[test]
    fn drops_a_cached_body_the_loader_rejects_after_304() {
        let fixture = Fixture::new();
        let dir = fixture.path("http-cache");
        let (url, server) = serve_responses(vec![
            not_modified_response(),
            ok_response("\"fresh\"", FRESH_BODY),
        ]);
        let entry = CacheEntry::for_url(&dir, &url);
        entry.write("\"stale\"", CACHED_BODY);

        let (rejected, rejected_bodies) = fetch(&url, &dir, false);
        assert!(
            entry.read().is_none(),
            "a rejected cached body must not be revalidated again"
        );
        let (fresh, fresh_bodies) = fetch(&url, &dir, true);
        let requests = server
            .join()
            .expect("test server")
            .expect("server response");

        rejected.expect("a 304 for a rejected body is still a response");
        fresh.expect("the next fetch should succeed");
        assert_eq!(rejected_bodies, [CACHED_BODY]);
        assert_eq!(fresh_bodies, [FRESH_BODY]);
        assert!(
            requests[0]
                .to_ascii_lowercase()
                .contains("if-none-match: \"stale\"")
        );
        assert!(!requests[1].to_ascii_lowercase().contains("if-none-match"));
        let cached = entry.read().expect("fresh cache entry");
        assert_eq!(cached.etag, "\"fresh\"");
        assert_eq!(cached.body, FRESH_BODY);
    }

    #[test]
    fn does_not_cache_a_200_body_the_loader_rejects() {
        let fixture = Fixture::new();
        let dir = fixture.path("http-cache");
        let (url, server) = serve_responses(vec![
            ok_response("\"unusable\"", CACHED_BODY),
            ok_response("\"fresh\"", FRESH_BODY),
        ]);

        let (rejected, rejected_bodies) = fetch(&url, &dir, false);
        assert!(CacheEntry::for_url(&dir, &url).read().is_none());
        let (fresh, fresh_bodies) = fetch(&url, &dir, true);
        let requests = server
            .join()
            .expect("test server")
            .expect("server response");

        rejected.expect("a rejected body is still a response");
        fresh.expect("the next fetch should succeed");
        assert_eq!(rejected_bodies, [CACHED_BODY]);
        assert_eq!(fresh_bodies, [FRESH_BODY]);
        assert_eq!(requests.len(), 2);
        assert!(
            !requests[1].to_ascii_lowercase().contains("if-none-match"),
            "no conditional request may follow a rejected body"
        );
        let cached = CacheEntry::for_url(&dir, &url)
            .read()
            .expect("accepted body is cached");
        assert_eq!(cached.etag, "\"fresh\"");
        assert_eq!(cached.body, FRESH_BODY);
    }

    #[test]
    fn transport_errors_do_not_reuse_cached_body() {
        let fixture = Fixture::new();
        let dir = fixture.path("http-cache");
        let (url, server) = close_after_request();
        let entry = CacheEntry::for_url(&dir, &url);
        entry.write("\"cached\"", CACHED_BODY);

        let (result, bodies) = fetch(&url, &dir, true);
        assert!(
            result.is_err(),
            "a transport failure must not turn an unbounded-age cache into a response"
        );
        assert!(bodies.is_empty());
        let request = server.join().expect("test server").expect("server request");
        assert!(
            request
                .to_ascii_lowercase()
                .contains("if-none-match: \"cached\"")
        );
        let cached = entry.read().expect("transport failure keeps cache on disk");
        assert_eq!(cached.body, CACHED_BODY);
    }

    #[test]
    fn oversized_response_does_not_reuse_cached_body() {
        let fixture = Fixture::new();
        let dir = fixture.path("http-cache");
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\netag: \"oversized\"\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            PRICING_FETCH_MAX_BYTES + 1,
        );
        let (url, server) = serve_responses(vec![response]);
        let entry = CacheEntry::for_url(&dir, &url);
        entry.write("\"cached\"", CACHED_BODY);

        let (result, bodies) = fetch(&url, &dir, true);
        assert!(
            result.is_err(),
            "an oversized response must not reuse cached pricing"
        );
        assert!(bodies.is_empty());
        let _ = server
            .join()
            .expect("test server")
            .expect("server response");
        assert_eq!(entry.read().expect("cache remains").body, CACHED_BODY);
    }

    #[test]
    fn bounded_body_reader_rejects_data_over_its_limit() {
        let error = read_bounded_body(Cursor::new("12345"), 4).expect_err("body is oversized");
        assert_eq!(error.kind(), io::ErrorKind::Other);
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
                    .all(|character| character.is_ascii_alphanumeric()
                        || matches!(character, '.' | '-' | '_')),
                "stem {stem} must be filesystem-safe"
            );
        }
        assert_ne!(
            cache_file_stem(&format!("https://example.com/{}/a.json", "x".repeat(200))),
            cache_file_stem(&format!("https://example.com/{}/b.json", "x".repeat(200))),
        );
    }

    #[test]
    fn cache_dir_prefers_xdg_then_home() {
        assert_eq!(
            cache_dir_under(Some(PathBuf::from("/xdg")), Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/xdg/ccusage/http-cache")),
        );
        assert_eq!(
            cache_dir_under(None, Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from("/home/u/.cache/ccusage/http-cache")),
        );
        assert_eq!(cache_dir_under(Some(PathBuf::from("relative")), None), None);
    }
}
