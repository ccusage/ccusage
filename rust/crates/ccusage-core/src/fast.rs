use std::io::{self, Read};

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
pub struct LinePrefilter {
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
    pub fn all(markers: &[&[u8]]) -> Self {
        Self::new(markers, PrefilterMode::All)
    }

    /// Build a prefilter that admits lines containing *any* of `markers`.
    pub fn any(markers: &[&[u8]]) -> Self {
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
    pub fn matches(&self, line: &[u8]) -> bool {
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
