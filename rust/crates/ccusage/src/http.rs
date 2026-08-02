use std::{
    fs, io,
    path::{Path, PathBuf},
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
/// instead of the full download. GitHub serves the LiteLLM pricing table with a
/// strong ETag, so a 1-minute polling loop drops from ~1.7MB per run to a few
/// hundred bytes. The cache is best-effort like the statusline cache: a missing
/// or torn entry just means a full fetch.
pub(crate) fn fetch_json(url: &str) -> io::Result<String> {
    fetch_json_with_cache_dir(url, &default_cache_dir())
}

fn default_cache_dir() -> PathBuf {
    std::env::temp_dir().join("ccusage-http-cache")
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
        // A failed revalidation must not take down a run that still has a
        // previously validated body on disk.
        Err(error) => {
            return match cached {
                Some(cached) => Ok(cached.body),
                None => Err(io::Error::other(error.to_string())),
            };
        }
    };
    if response.status().as_u16() == 304
        && let Some(cached) = cached
    {
        return Ok(cached.body);
    }
    if response.status().as_u16() != 200 {
        return Err(io::Error::other(format!(
            "HTTP {}",
            response.status().as_u16()
        )));
    }
    let etag = response
        .headers()
        .get("etag")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = response
        .body_mut()
        .with_config()
        .limit(PRICING_FETCH_MAX_BYTES)
        .read_to_string()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    if let Some(etag) = etag {
        cache.write(&etag, &body);
    }
    Ok(body)
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
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        // Write to a per-process temp file and rename it into place: rename
        // within a directory is atomic, so readers see either the old pair or
        // the new pair, never a torn or mixed one.
        let tmp_path = self
            .path
            .with_extension(format!("tmp{}", std::process::id()));
        if fs::write(&tmp_path, format!("{etag}\n{body}")).is_ok()
            && fs::rename(&tmp_path, &self.path).is_err()
        {
            let _ = fs::remove_file(&tmp_path);
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
    use super::{CacheEntry, cache_file_stem};
    use std::fs;

    fn unique_test_dir(label: &str) -> std::path::PathBuf {
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
}
