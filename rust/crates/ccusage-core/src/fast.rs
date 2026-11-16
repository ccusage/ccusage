use std::io::{self, Read};

use memchr::{memchr, memchr_iter, memmem::Finder};
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
/// a marker only in escaped form is still admitted. A marker with a bracket
/// outside its strings is found, verbatim or escaped, in one pass over that
/// bracket. A line that misses any other marker verbatim pays one more scan,
/// for an escape that can spell a marker byte, and only a line that holds such
/// an escape pays for more.
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
            let found = match &marker.anchor {
                Some(anchor) => marker.spelled_at_anchor(line, anchor),
                None => marker.finder.find(line).is_some(),
            };
            if found {
                if matches!(self.mode, PrefilterMode::Any) {
                    return true;
                }
            } else if marker.anchor.is_some() {
                if matches!(self.mode, PrefilterMode::All) {
                    return false;
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
            PrefilterMode::Any => missing != 0 && self.spells_escaped(line, missing, false),
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
            if preceded_by_paired_backslashes(line, start) {
                return Some(start);
            }
            from = start + 2;
        }
    }
}

/// Whether a backslash at `start` begins an escape: the backslashes before it
/// must themselves be escaped in pairs.
fn preceded_by_paired_backslashes(line: &[u8], start: usize) -> bool {
    line[..start]
        .iter()
        .rev()
        .take_while(|&&byte| byte == b'\\')
        .count()
        % 2
        == 0
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
    anchor: Option<Anchor>,
}

/// A bracket outside the marker's strings. JSON cannot escape it, so every
/// spelling of the marker, verbatim or escaped, holds it verbatim, and lines
/// hold far fewer brackets than backslashes: one `memchr` pass for it finds
/// every candidate spelling.
struct Anchor {
    byte: u8,
    /// Where the bracket sits in the marker.
    index: usize,
    /// Where the structural bytes before the bracket start in the marker.
    run_start: usize,
    /// The most line bytes the marker before `run_start` can take, with every
    /// content byte spelled as a six-byte `\u` escape.
    reach: usize,
}

impl Anchor {
    fn new(marker: &[u8], content: &[bool]) -> Option<Self> {
        let index = marker
            .iter()
            .zip(content)
            .position(|(byte, is_content)| !is_content && b"{}[]".contains(byte))?;
        let run_start = content[..index]
            .iter()
            .rposition(|&is_content| is_content)
            .map_or(0, |content| content + 1);
        Some(Self {
            byte: marker[index],
            index,
            run_start,
            reach: content[..run_start]
                .iter()
                .map(|&is_content| if is_content { 6 } else { 1 })
                .sum(),
        })
    }
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
            anchor: Anchor::new(marker, &content),
            content,
            escapable,
        }
    }

    /// Whether `line` spells the marker, verbatim or with escapes, around an
    /// occurrence of its anchor.
    fn spelled_at_anchor(&self, line: &[u8], anchor: &Anchor) -> bool {
        let marker = self.finder.needle();
        let run = &marker[anchor.run_start..anchor.index];
        for at in memchr_iter(anchor.byte, line) {
            let Some(run_at) = at.checked_sub(run.len()) else {
                continue;
            };
            if !line[run_at..at].iter().eq(run) || !self.matches_at(anchor.index, &line[at..]) {
                continue;
            }
            let verbatim = run_at
                .checked_sub(anchor.run_start)
                .is_some_and(|start| line[start..run_at].iter().eq(&marker[..anchor.run_start]));
            // Escaping a byte before the run leaves a backslash within reach.
            if verbatim
                || line[run_at.saturating_sub(anchor.reach)..run_at].contains(&b'\\')
                    && self.spelled_before(line, anchor.run_start, run_at)
            {
                return true;
            }
        }
        false
    }

    /// Whether `line[..end]` ends with a spelling of the marker's first
    /// `index` bytes. An escape that spells the first byte only counts when
    /// the backslashes before it pair up.
    fn spelled_before(&self, line: &[u8], index: usize, end: usize) -> bool {
        let Some(index) = index.checked_sub(1) else {
            return true;
        };
        let expected = self.finder.needle()[index];
        if end > 0 && line[end - 1] == expected && self.spelled_before(line, index, end - 1) {
            return true;
        }
        self.content[index]
            && [2, 6].into_iter().any(|len| {
                end.checked_sub(len).is_some_and(|start| {
                    line[start] == b'\\'
                        && decode_escape(&line[start..end]) == (Some(expected), len)
                        && (index > 0 || preceded_by_paired_backslashes(line, start))
                        && self.spelled_before(line, index, start)
                })
            })
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

/// Splits a reader into `\n`-terminated lines through one reusable buffer.
///
/// Unlike reading a whole log and running [`byte_lines`] over it, only the
/// current window of the file is resident, so the bytes a parser touches are
/// still in cache from the `read` that brought them in.
pub struct LineReader<R> {
    reader: R,
    buffer: Vec<u8>,
    start: usize,
    scanned: usize,
    end: usize,
    eof: bool,
}

impl<R: Read> LineReader<R> {
    pub fn with_capacity(capacity: usize, reader: R) -> Self {
        Self {
            reader,
            buffer: vec![0; capacity.max(1)],
            start: 0,
            scanned: 0,
            end: 0,
            eof: false,
        }
    }

    /// The next line, including its `\n` unless it is the unterminated last line.
    ///
    /// A read error leaves the bytes after the last returned line in
    /// [`Self::rest`].
    pub fn next_line(&mut self) -> io::Result<Option<&[u8]>> {
        let line_end = loop {
            if let Some(newline) = memchr(b'\n', &self.buffer[self.scanned..self.end]) {
                break self.scanned + newline + 1;
            }
            self.scanned = self.end;
            if self.eof {
                if self.start == self.end {
                    return Ok(None);
                }
                break self.end;
            }
            self.fill()?;
        };
        let line = &self.buffer[self.start..line_end];
        self.start = line_end;
        self.scanned = line_end;
        Ok(Some(line))
    }

    /// Bytes received after the last line [`Self::next_line`] returned.
    pub fn rest(&self) -> &[u8] {
        &self.buffer[self.start..self.end]
    }

    fn fill(&mut self) -> io::Result<()> {
        if self.start > 0 {
            self.buffer.copy_within(self.start..self.end, 0);
            self.end -= self.start;
            self.scanned -= self.start;
            self.start = 0;
        }
        if self.end == self.buffer.len() {
            self.buffer.resize(self.buffer.len() * 2, 0);
        }
        loop {
            match self.reader.read(&mut self.buffer[self.end..]) {
                Ok(0) => {
                    self.eof = true;
                    return Ok(());
                }
                Ok(read) => {
                    self.end += read;
                    return Ok(());
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }
}

pub fn suffix_string(value: &str, suffix: &str) -> String {
    let mut output = String::with_capacity(value.len() + suffix.len());
    output.push_str(value);
    output.push_str(suffix);
    output
}

#[cfg(test)]
mod tests {
    use std::io::{self, Cursor, Read};

    use super::{LinePrefilter, LineReader, byte_lines, suffix_string};

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

    /// A `\u` escape for `hex`, built at runtime so the test source holds no
    /// escape that an editor or formatter could decode.
    fn unicode_escape(hex: &str) -> String {
        ["\\", "u", hex].concat()
    }

    #[test]
    fn line_prefilter_finds_bracketed_markers_through_their_bracket() {
        let prefilter = LinePrefilter::all(&[br#""usage":{"#]);
        let u = unicode_escape("0075");
        let a = unicode_escape("0061");
        let e = unicode_escape("0065");

        for line in [
            r#"{"message":{"usage":{}}}"#.to_owned(),
            format!(r#"{{"message":{{"{u}sage":{{}}}}}}"#),
            format!(r#"{{"message":{{"us{a}g{e}":{{}}}}}}"#),
            format!(r#"{{"n":"\\","us{a}ge":{{}}}}"#),
        ] {
            assert!(prefilter.matches(line.as_bytes()), "{line}");
        }
        for line in [
            r#"{"message":{"usage" :{}}}"#.to_owned(),
            r#"{"text":"\"usage\":{"}"#.to_owned(),
            format!(r#"{{"us\\{a}ge":{{}}}}"#),
            format!(r#"{{"us{a}ge":[]}}"#),
            format!(r#"{{us{a}ge":{{}}}}"#),
            format!(r#"{{"{}":{{}}}}"#, unicode_escape("00e9")),
        ] {
            assert!(!prefilter.matches(line.as_bytes()), "{line}");
        }
    }

    #[test]
    fn line_prefilter_follows_escapes_after_a_bracket() {
        let prefilter = LinePrefilter::any(&[br#""k":{"a/b"#]);

        assert!(prefilter.matches(br#"{"k":{"a\/b":1}}"#));
        assert!(
            prefilter
                .matches(format!(r#"{{"k":{{"{}/b":1}}}}"#, unicode_escape("0061")).as_bytes())
        );
        assert!(!prefilter.matches(br#"{"k":{"a\\/b":1}}"#));
        assert!(!prefilter.matches(br#"{"k":{ "a/b":1}}"#));
    }

    /// How many escapes each spelling of `marker` that starts at `line[start]`
    /// takes, with every string byte verbatim or escaped. This brute force is
    /// the definition the prefilter's searches must agree with.
    fn spellings(marker: &[u8], line: &[u8], start: usize) -> Vec<usize> {
        let mut in_string = marker.first() != Some(&b'"');
        let content = marker
            .iter()
            .map(|&byte| {
                in_string ^= byte == b'"';
                byte != b'"' && in_string
            })
            .collect::<Vec<_>>();
        let mut found = Vec::new();
        let mut pending = vec![(0, start, 0)];
        while let Some((index, at, escapes)) = pending.pop() {
            if index == marker.len() {
                found.push(escapes);
                continue;
            }
            if line.get(at) == Some(&marker[index]) {
                pending.push((index + 1, at + 1, escapes));
            }
            let real_escape = index > 0
                || line[..at]
                    .iter()
                    .rev()
                    .take_while(|&&byte| byte == b'\\')
                    .count()
                    % 2
                    == 0;
            if !content[index] || line.get(at) != Some(&b'\\') || !real_escape {
                continue;
            }
            let short = match line.get(at + 1) {
                Some(b'"') => Some(b'"'),
                Some(b'\\') => Some(b'\\'),
                Some(b'/') => Some(b'/'),
                Some(b'b') => Some(0x08),
                Some(b'f') => Some(0x0c),
                Some(b'n') => Some(b'\n'),
                Some(b'r') => Some(b'\r'),
                Some(b't') => Some(b'\t'),
                _ => None,
            };
            if short == Some(marker[index]) {
                pending.push((index + 1, at + 2, escapes + 1));
            }
            let unicode = line
                .get(at + 1..at + 6)
                .filter(|escape| escape[0] == b'u')
                .and_then(|escape| std::str::from_utf8(&escape[1..]).ok())
                .filter(|hex| hex.bytes().all(|digit| digit.is_ascii_hexdigit()))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok());
            if unicode == Some(u32::from(marker[index])) {
                pending.push((index + 1, at + 6, escapes + 1));
            }
        }
        found
    }

    fn spells(marker: &[u8], line: &[u8], escaped_only: bool) -> bool {
        (0..=line.len()).any(|start| {
            spellings(marker, line, start)
                .into_iter()
                .any(|escapes| !escaped_only || escapes > 0)
        })
    }

    #[test]
    fn line_prefilter_admits_exactly_the_lines_that_spell_its_markers() {
        let mut fragments = [
            &b"\""[..],
            b":",
            b"{",
            b"}",
            b"[",
            b"]",
            b",",
            b" ",
            b"\\",
            b"\\\\",
            br"\/",
            b"/",
            br"\n",
            br#"\""#,
            b"u",
            b"s",
            b"a",
            b"g",
            b"e",
            b"usage",
            b"type",
            b"x",
            b"_",
        ]
        .map(<[u8]>::to_vec)
        .to_vec();
        for hex in [
            "0075", "0061", "0065", "0045", "005f", "005F", "002f", "0078", "00e9", "00", "006",
        ] {
            fragments.push(unicode_escape(hex).into_bytes());
        }
        let marker_sets: [&[&[u8]]; 7] = [
            &[br#""usage":{"#],
            &[br#""usage":{"#, br#""type""#],
            &[br#""x":{"a/e"#, b"us_e"],
            &[br#""a":[]"#],
            &[br#"]"x/a""#, b"a/e"],
            &[br#"{"a":["#],
            &[br#"{"#, br#""a_x""#],
        ];
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = move |bound: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            usize::try_from(state % bound as u64).unwrap_or_default()
        };
        for markers in marker_sets {
            let all = LinePrefilter::all(markers);
            let any = LinePrefilter::any(markers);
            for _ in 0..5_000 {
                let line = (0..next(16))
                    .flat_map(|_| fragments[next(fragments.len())].clone())
                    .collect::<Vec<_>>();
                let text = String::from_utf8_lossy(&line);
                let spelled = markers.iter().map(|marker| spells(marker, &line, false));
                assert_eq!(all.matches(&line), spelled.clone().all(|s| s), "all {text}");
                assert_eq!(any.matches(&line), spelled.clone().any(|s| s), "any {text}");
                assert_eq!(
                    any.matches_escaped(&line),
                    markers.iter().any(|marker| spells(marker, &line, true)),
                    "escaped {text}"
                );
            }
        }
    }

    #[test]
    fn byte_lines_returns_newline_delimited_slices() {
        let lines = byte_lines(b"one\ntwo\nthree").collect::<Vec<_>>();

        assert_eq!(
            lines,
            [b"one".as_slice(), b"two".as_slice(), b"three".as_slice()]
        );
    }

    fn read_lines(content: &[u8], capacity: usize) -> Vec<Vec<u8>> {
        let mut reader = LineReader::with_capacity(capacity, Cursor::new(content));
        let mut lines = Vec::new();
        while let Some(line) = reader.next_line().unwrap() {
            lines.push(line.to_vec());
        }
        lines
    }

    #[test]
    fn line_reader_matches_byte_lines_across_buffer_boundaries() {
        let content = b"one\n\ntwo lines longer than the buffer\nthree";
        for capacity in [1, 2, 3, 7, 64] {
            let lines = read_lines(content, capacity);
            let stripped = lines
                .iter()
                .map(|line| line.strip_suffix(b"\n").unwrap_or(line))
                .collect::<Vec<_>>();
            assert_eq!(stripped, byte_lines(content).collect::<Vec<_>>());
            assert_eq!(lines.concat(), content);
        }
        assert!(read_lines(b"", 4).is_empty());
        assert_eq!(read_lines(b"one\n", 4), [b"one\n".to_vec()]);
    }

    struct Failing;

    impl Read for Failing {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("read failed"))
        }
    }

    #[test]
    fn line_reader_keeps_the_partial_line_before_a_read_error() {
        let mut reader =
            LineReader::with_capacity(4, Cursor::new(b"complete\npartial").chain(Failing));
        assert_eq!(reader.next_line().unwrap(), Some(b"complete\n".as_slice()));
        assert!(reader.next_line().is_err());
        assert_eq!(reader.rest(), b"partial");
    }

    #[test]
    fn suffix_string_builds_without_formatting() {
        assert_eq!(
            suffix_string("claude-sonnet-4", "-fast"),
            "claude-sonnet-4-fast"
        );
    }
}
