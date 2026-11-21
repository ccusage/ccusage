use memchr::{memchr, memchr_iter, memmem::Finder};
use smallvec::SmallVec;

#[cfg(test)]
mod reference;

#[derive(Clone, Copy)]
enum PrefilterMode {
    All,
    Any,
}

/// Byte-substring prefilter for JSONL records. Markers inside JSON strings
/// also match their escaped spellings.
pub struct LinePrefilter {
    markers: SmallVec<[Marker; 4]>,
    escapable: u128,
    short_escapable: bool,
    mode: PrefilterMode,
}

impl LinePrefilter {
    pub fn all(markers: &[&[u8]]) -> Self {
        Self::new(markers, PrefilterMode::All)
    }

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

    /// Matches a marker only when at least one of its bytes is JSON-escaped.
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

fn preceded_by_paired_backslashes(line: &[u8], start: usize) -> bool {
    line[..start]
        .iter()
        .rev()
        .take_while(|&&byte| byte == b'\\')
        .count()
        % 2
        == 0
}

const SHORT_ESCAPE_BYTES: [u8; 8] = [b'"', b'\\', b'/', 0x08, 0x0c, b'\n', b'\r', b'\t'];

static ASCII_UNICODE_ESCAPE: std::sync::LazyLock<Finder<'static>> =
    std::sync::LazyLock::new(|| Finder::new(br"\u00"));

struct Marker {
    finder: Finder<'static>,
    content: Box<[bool]>,
    escapable: u128,
    anchor: Option<Anchor>,
}

// NOTE(2026-11-17): RFC 8259 §7 limits JSON escapes to strings. A bracket outside a string can therefore anchor every spelling of a marker.
// NOTE(2026-11-21, #2367): RFC 8259 §2 allows whitespace around structural tokens. The run before the bracket is therefore matched token by token, not as one literal.
struct Anchor {
    index: usize,
    run_start: usize,
    before_bracket: SmallVec<[Token; 4]>,
}

struct Token {
    byte: u8,
    spaced: bool,
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
        let before_bracket = marker[run_start..=index]
            .windows(2)
            .rev()
            .map(|pair| Token {
                byte: pair[0],
                spaced: pair.iter().any(|byte| b":,{}[]".contains(byte)),
            })
            .collect();
        Some(Self {
            index,
            run_start,
            before_bracket,
        })
    }

    fn run_at(&self, line: &[u8], bracket_at: usize) -> Option<usize> {
        let mut end = bracket_at;
        for token in &self.before_bracket {
            if token.spaced {
                while end > 0 && matches!(line[end - 1], b' ' | b'\t' | b'\n' | b'\r') {
                    end -= 1;
                }
            }
            end = end.checked_sub(1).filter(|&at| line[at] == token.byte)?;
        }
        Some(end)
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
            finder: Finder::new(marker).into_owned(),
            anchor: Anchor::new(marker, &content),
            content,
            escapable,
        }
    }

    fn spelled_at_anchor(&self, line: &[u8], anchor: &Anchor) -> bool {
        let marker = self.finder.needle();
        let mut first_backslash = None;
        for bracket_at in memchr_iter(marker[anchor.index], line) {
            if anchor.index + 1 < marker.len()
                && !self.matches_at(anchor.index + 1, &line[bracket_at + 1..])
            {
                continue;
            }
            let Some(run_at) = anchor.run_at(line, bracket_at) else {
                continue;
            };
            let verbatim = run_at
                .checked_sub(anchor.run_start)
                .is_some_and(|start| line[start..run_at] == marker[..anchor.run_start]);
            if verbatim
                || first_backslash
                    .get_or_insert_with(|| memchr(b'\\', line))
                    .is_some_and(|backslash| backslash < run_at)
                    && self.spelled_before(line, anchor.run_start, run_at)
            {
                return true;
            }
        }
        false
    }

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

#[cfg(test)]
mod tests;
