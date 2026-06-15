use memchr::{memchr, memmem::Finder};
use serde_json::Value;
use smallvec::SmallVec;

pub(crate) type FxHashMap<K, V> = rustc_hash::FxHashMap<K, V>;
pub(crate) type FxHashSet<T> = rustc_hash::FxHashSet<T>;
pub(crate) type SmallIndexVec = SmallVec<[usize; 1]>;

/// Whether a [`LinePrefilter`] requires every marker or just one of them.
#[derive(Clone, Copy)]
enum PrefilterMode {
    /// The line must contain every configured marker.
    All,
    /// The line must contain at least one configured marker.
    Any,
}

/// Reusable byte-substring prefilter for newline-delimited JSON logs.
///
/// JSONL adapters skip lines that cannot contain a usage record before paying
/// for a full `serde_json` parse. Building the [`Finder`] needles once and
/// reusing them across every line keeps that skip check on the SIMD-accelerated
/// `memmem` path instead of allocating a fresh searcher per `str::contains`
/// call.
pub(crate) struct LinePrefilter {
    finders: SmallVec<[Finder<'static>; 4]>,
    mode: PrefilterMode,
}

impl LinePrefilter {
    /// Build a prefilter that only admits lines containing *all* `markers`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let prefilter = LinePrefilter::all(&[b"\"usage\"", b"\"message\""]);
    /// assert!(prefilter.matches(br#"{"message":{"usage":{}}}"#));
    /// assert!(!prefilter.matches(br#"{"message":{}}"#));
    /// ```
    pub(crate) fn all(markers: &[&[u8]]) -> Self {
        Self::new(markers, PrefilterMode::All)
    }

    /// Build a prefilter that admits lines containing *any* of `markers`.
    pub(crate) fn any(markers: &[&[u8]]) -> Self {
        Self::new(markers, PrefilterMode::Any)
    }

    fn new(markers: &[&[u8]], mode: PrefilterMode) -> Self {
        // `Finder::new` borrows the needle, so take an owned copy to outlive the
        // caller's marker slice and allow the prefilter to be stored freely.
        let finders = markers
            .iter()
            .map(|marker| Finder::new(marker).into_owned())
            .collect();
        Self { finders, mode }
    }

    /// Return `true` when `line` passes the filter and is worth parsing.
    pub(crate) fn matches(&self, line: &[u8]) -> bool {
        match self.mode {
            PrefilterMode::All => self
                .finders
                .iter()
                .all(|finder| finder.find(line).is_some()),
            PrefilterMode::Any => self
                .finders
                .iter()
                .any(|finder| finder.find(line).is_some()),
        }
    }
}

pub(crate) struct ByteLines<'a> {
    bytes: &'a [u8],
}

impl<'a> ByteLines<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
}

impl<'a> Iterator for ByteLines<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        if self.bytes.is_empty() {
            return None;
        }
        if let Some(newline) = memchr(b'\n', self.bytes) {
            let (line, rest) = self.bytes.split_at(newline);
            self.bytes = &rest[1..];
            Some(line)
        } else {
            let line = self.bytes;
            self.bytes = &[];
            Some(line)
        }
    }
}

pub(crate) fn byte_lines(bytes: &[u8]) -> ByteLines<'_> {
    ByteLines::new(bytes)
}

/// Parse newline-delimited JSON, yielding each owned [`Value`] whose source
/// line passes `prefilter`.
///
/// This is the shared fast-path used by the line-delimited agent adapters: it
/// iterates the raw bytes with [`byte_lines`], skips lines rejected by the
/// reusable [`LinePrefilter`] before paying for a parse, and silently drops
/// lines that fail to deserialize (matching the per-adapter `let Ok(..) else`
/// behavior). Callers keep ownership of `content` and the `prefilter`.
///
/// # Example
///
/// ```ignore
/// let content = std::fs::read(path)?;
/// let prefilter = LinePrefilter::all(&[b"\"usage\""]);
/// for value in prefiltered_json_values(&content, &prefilter) {
///     // adapter-specific handling of `value`
/// }
/// ```
pub(crate) fn prefiltered_json_values<'a>(
    content: &'a [u8],
    prefilter: &'a LinePrefilter,
) -> impl Iterator<Item = Value> + 'a {
    byte_lines(content)
        .filter(move |line| prefilter.matches(line))
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
}

pub(crate) fn suffix_string(value: &str, suffix: &str) -> String {
    let mut output = String::with_capacity(value.len() + suffix.len());
    output.push_str(value);
    output.push_str(suffix);
    output
}

#[cfg(test)]
mod tests {
    use super::{LinePrefilter, byte_lines, prefiltered_json_values, suffix_string};

    #[test]
    fn prefiltered_json_values_skips_filtered_and_malformed_lines() {
        let content = concat!(
            r#"{"usage":{"input":1}}"#,
            "\n",
            r#"{"role":"user"}"#,
            "\n",
            r#"not json but has "usage""#,
            "\n",
            r#"{"usage":{"input":2}}"#,
        )
        .as_bytes();
        let prefilter = LinePrefilter::all(&[b"\"usage\""]);

        let inputs = prefiltered_json_values(content, &prefilter)
            .filter_map(|value| value.get("usage")?.get("input")?.as_u64())
            .collect::<Vec<_>>();

        assert_eq!(inputs, [1, 2]);
    }

    #[test]
    fn line_prefilter_all_requires_every_marker() {
        let prefilter = LinePrefilter::all(&[b"\"usage\"", b"\"message\""]);

        assert!(prefilter.matches(br#"{"message":{"usage":{"input":1}}}"#));
        assert!(!prefilter.matches(br#"{"message":{"role":"user"}}"#));
        assert!(!prefilter.matches(br#"{"usage":{"input":1}}"#));
    }

    #[test]
    fn line_prefilter_any_requires_one_marker() {
        let prefilter = LinePrefilter::any(&[b"\"model_change\"", b"\"usage\""]);

        assert!(prefilter.matches(br#"{"type":"model_change"}"#));
        assert!(prefilter.matches(br#"{"message":{"usage":{}}}"#));
        assert!(!prefilter.matches(br#"{"type":"message"}"#));
    }

    #[test]
    fn byte_lines_returns_newline_delimited_slices() {
        let lines = byte_lines(b"one\ntwo\nthree").collect::<Vec<_>>();

        assert_eq!(
            lines,
            [b"one".as_slice(), b"two".as_slice(), b"three".as_slice()]
        );
    }

    #[test]
    fn suffix_string_builds_without_formatting() {
        assert_eq!(
            suffix_string("claude-sonnet-4", "-fast"),
            "claude-sonnet-4-fast"
        );
    }
}
