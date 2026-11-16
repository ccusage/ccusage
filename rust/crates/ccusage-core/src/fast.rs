use memchr::{memchr, memmem::Finder};
use smallvec::SmallVec;

pub type FxHashMap<K, V> = rustc_hash::FxHashMap<K, V>;
pub type FxHashSet<T> = rustc_hash::FxHashSet<T>;

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
///
/// A marker is JSON text read from its first byte: bytes inside double quotes,
/// or every byte of a marker that does not start with one, are string content,
/// which JSON lets a writer spell as an escape such as `\/`. A line that holds
/// a marker only in escaped form is still admitted. A line that misses a marker
/// verbatim pays one more scan, for an escape that can spell a marker byte, and
/// only a line that holds such an escape pays for more.
pub struct LinePrefilter {
    markers: SmallVec<[Marker; 4]>,
    escapable: u128,
    /// Whether a marker byte can be spelled by a two-byte escape such as `\/`,
    /// not only by a `\u` escape.
    short_escapable: bool,
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
    pub fn all(markers: &[&[u8]]) -> Self {
        Self::new(markers, PrefilterMode::All)
    }

    /// Build a prefilter that admits lines containing *any* of `markers`.
    pub fn any(markers: &[&[u8]]) -> Self {
        Self::new(markers, PrefilterMode::Any)
    }

    fn new(markers: &[&[u8]], mode: PrefilterMode) -> Self {
        debug_assert!(markers.len() <= 64, "a prefilter tracks at most 64 markers");
        let markers = markers
            .iter()
            .map(|marker| Marker::new(marker))
            .collect::<SmallVec<[Marker; 4]>>();
        let escapable = markers.iter().fold(0, |set, marker| set | marker.escapable);
        let short_escapable = SHORT_ESCAPE_BYTES
            .iter()
            .any(|&byte| escapable >> byte & 1 == 1);
        Self {
            markers,
            escapable,
            short_escapable,
            mode,
        }
    }

    /// Return `true` when `line` passes the filter and is worth parsing.
    pub fn matches(&self, line: &[u8]) -> bool {
        let mut has_escape = None;
        let mut missing = 0_u64;
        for (index, marker) in self.markers.iter().enumerate() {
            if marker.finder.find(line).is_some() {
                if matches!(self.mode, PrefilterMode::Any) {
                    return true;
                }
            } else if matches!(self.mode, PrefilterMode::All)
                && !*has_escape.get_or_insert_with(|| self.next_escape(line, 0).is_some())
            {
                return false;
            } else {
                missing |= 1 << index;
            }
        }
        match self.mode {
            PrefilterMode::All => missing == 0 || self.spells_escaped(line, missing, true),
            PrefilterMode::Any => self.spells_escaped(line, missing, false),
        }
    }

    /// Return `true` when `line` spells one of the markers with at least one
    /// escape, whether or not it also holds one verbatim.
    pub fn matches_escaped(&self, line: &[u8]) -> bool {
        let every = (1_u128 << self.markers.len()) - 1;
        self.spells_escaped(line, every as u64, false)
    }

    fn spells_escaped(&self, line: &[u8], mut pending: u64, all: bool) -> bool {
        let mut from = 0;
        while let Some(start) = self.next_escape(line, from) {
            let (decoded, len) = decode_escape(&line[start..]);
            from = start + len;
            let Some(byte) = decoded.filter(|&byte| self.escapable >> byte & 1 == 1) else {
                continue;
            };
            for (index, marker) in self.markers.iter().enumerate() {
                if pending >> index & 1 == 1 && marker.spelled_around(line, start, from, byte) {
                    pending &= !(1 << index);
                    if !all || pending == 0 {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// The start of the next escape at or after `from` that can spell a marker
    /// byte. Tool output is full of `\n`, `\"` and `\\` escapes, so when only a
    /// unicode escape can spell a marker byte the search jumps between `\u00`
    /// escapes instead of stopping at every backslash.
    fn next_escape(&self, line: &[u8], mut from: usize) -> Option<usize> {
        if self.short_escapable {
            return memchr(b'\\', &line[from..]).map(|offset| from + offset);
        }
        loop {
            let start = from + ASCII_UNICODE_ESCAPE.find(&line[from..])?;
            // `\u` starts an escape only when the backslashes before it are
            // themselves escaped in pairs.
            let preceding = line[..start]
                .iter()
                .rev()
                .take_while(|&&byte| byte == b'\\')
                .count();
            if preceding % 2 == 0 {
                return Some(start);
            }
            from = start + 2;
        }
    }
}

/// Bytes a two-byte JSON escape decodes to.
const SHORT_ESCAPE_BYTES: [u8; 8] = [b'"', b'\\', b'/', 0x08, 0x0c, b'\n', b'\r', b'\t'];

/// The start of every `\u` escape that decodes to an ASCII byte, the only ones
/// that can spell a marker byte.
static ASCII_UNICODE_ESCAPE: std::sync::LazyLock<Finder<'static>> =
    std::sync::LazyLock::new(|| Finder::new(br"\u00"));

struct Marker {
    finder: Finder<'static>,
    content: Box<[bool]>,
    escapable: u128,
}

impl Marker {
    fn new(marker: &[u8]) -> Self {
        debug_assert!(
            marker.is_ascii() && !marker.contains(&b'\\'),
            "a prefilter marker is ASCII JSON text without escapes"
        );
        let mut in_string = marker.first() != Some(&b'"');
        let content = marker
            .iter()
            .map(|&byte| {
                if byte == b'"' {
                    in_string = !in_string;
                    false
                } else {
                    in_string
                }
            })
            .collect::<Box<[bool]>>();
        let escapable = marker
            .iter()
            .zip(&content)
            .filter(|(_, is_content)| **is_content)
            .fold(0, |set, (&byte, _)| set | 1_u128 << byte);
        Self {
            // `Finder::new` borrows the needle, so take an owned copy to outlive
            // the caller's marker slice and allow the prefilter to be stored freely.
            finder: Finder::new(marker).into_owned(),
            content,
            escapable,
        }
    }

    fn spelled_around(&self, line: &[u8], start: usize, end: usize, byte: u8) -> bool {
        let marker = self.finder.needle();
        self.escapable >> byte & 1 == 1
            && (0..marker.len().min(start + 1)).any(|index| {
                marker[index] == byte
                    && self.content[index]
                    && line[start - index..start] == marker[..index]
                    && self.matches_at(index + 1, &line[end..])
            })
    }

    fn matches_at(&self, index: usize, mut rest: &[u8]) -> bool {
        let marker = self.finder.needle();
        for (&expected, &is_content) in marker[index..].iter().zip(&self.content[index..]) {
            let len = match rest.first() {
                Some(b'\\') if is_content => match decode_escape(rest) {
                    (Some(byte), len) if byte == expected => len,
                    _ => return false,
                },
                Some(&byte) if byte == expected => 1,
                _ => return false,
            };
            rest = &rest[len..];
        }
        true
    }
}

fn decode_escape(escape: &[u8]) -> (Option<u8>, usize) {
    let decoded = match escape.get(1) {
        Some(b'"') => b'"',
        Some(b'\\') => b'\\',
        Some(b'/') => b'/',
        Some(b'b') => 0x08,
        Some(b'f') => 0x0c,
        Some(b'n') => b'\n',
        Some(b'r') => b'\r',
        Some(b't') => b'\t',
        Some(b'u') => {
            let Some(hex) = escape
                .get(2..6)
                .filter(|hex| hex.iter().all(u8::is_ascii_hexdigit))
            else {
                return (None, 2);
            };
            let code = hex.iter().fold(0_u32, |code, &digit| {
                code << 4 | char::from(digit).to_digit(16).unwrap_or_default()
            });
            return (u8::try_from(code).ok().filter(u8::is_ascii), 6);
        }
        Some(_) => return (None, 2),
        None => return (None, 1),
    };
    (Some(decoded), 2)
}

pub struct ByteLines<'a> {
    bytes: &'a [u8],
}

impl<'a> ByteLines<'a> {
    fn new(bytes: &'a [u8]) -> Self {
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

pub fn byte_lines(bytes: &[u8]) -> ByteLines<'_> {
    ByteLines::new(bytes)
}

pub fn suffix_string(value: &str, suffix: &str) -> String {
    let mut output = String::with_capacity(value.len() + suffix.len());
    output.push_str(value);
    output.push_str(suffix);
    output
}

#[cfg(test)]
mod tests {
    use super::{LinePrefilter, byte_lines, suffix_string};

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
    fn line_prefilter_admits_markers_spelled_with_json_escapes() {
        let bare = LinePrefilter::any(&[b"assistant/message"]);
        let quoted = LinePrefilter::all(&[br#""usage":{"#]);

        assert!(bare.matches(br#"{"type":"assistant\/message"}"#));
        assert!(bare.matches(br#"{"type":"\u0061ssistant\u002fmessage"}"#));
        assert!(bare.matches(br#"{"type":"assistant/messag\u0065"}"#));
        assert!(quoted.matches(br#"{"message":{"us\u0061ge":{"input":1}}}"#));
        assert!(quoted.matches(br#"{"message":{"\u0075sage":{}}}"#));
    }

    #[test]
    fn line_prefilter_rejects_escapes_that_do_not_spell_a_marker() {
        let bare = LinePrefilter::any(&[b"assistant/message"]);
        let quoted = LinePrefilter::all(&[br#""usage":{"#]);

        assert!(!bare.matches(br#"{"type":"tool","text":"a\\\/b \u001b[1m\n\"x\""}"#));
        assert!(!bare.matches(br#"{"type":"assistant\/messenger","t":"assistant\\/message"}"#));
        assert!(!bare.matches(br#"{"type":"assistant\u00e9message"}"#));
        assert!(!quoted.matches(br#"{"text":"\"usage\":{"}"#));
        assert!(!quoted.matches(br#"{"message":{"us\u0061ge":1}}"#));
        assert!(!quoted.matches(br#"{"message":{"us\u0061g":{}}}"#));
    }

    #[test]
    fn line_prefilter_all_admits_each_marker_verbatim_or_escaped() {
        let prefilter = LinePrefilter::all(&[br#""a/b""#, br#""c/d""#, br#""e""#]);

        assert!(prefilter.matches(br#"{"a\/b":1,"c\/d":2,"e":3}"#));
        assert!(!prefilter.matches(br#"{"a\/b":1,"e":3}"#));
    }

    #[test]
    fn line_prefilter_reads_a_unicode_escape_only_after_paired_backslashes() {
        let prefilter = LinePrefilter::all(&[br#""usage":{"#]);

        assert!(prefilter.matches(br#"{"n":"\\","us\u0061ge":{}}"#));
        assert!(prefilter.matches(br#"{"n":"\\\\","us\u0061ge":{}}"#));
        assert!(!prefilter.matches(br#"{"us\\u0061ge":{}}"#));
        assert!(!prefilter.matches(br#"{"n":"\n\"\t","x":{}}"#));
    }

    #[test]
    fn line_prefilter_follows_short_escapes_when_a_marker_holds_a_slash() {
        let prefilter = LinePrefilter::any(&[b"a/b"]);

        assert!(prefilter.matches(br#"{"t":"\n a\/b"}"#));
        assert!(prefilter.matches(br#"{"t":"a\u002fb"}"#));
        assert!(!prefilter.matches(br#"{"t":"a\\/b"}"#));
    }

    #[test]
    fn line_prefilter_matches_escaped_ignores_verbatim_markers() {
        let prefilter = LinePrefilter::any(&[b"a/b", b"c/d"]);

        assert!(prefilter.matches_escaped(br#"{"t":"a/b","u":"c\/d"}"#));
        assert!(!prefilter.matches_escaped(br#"{"t":"a/b","n":"\n"}"#));
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
