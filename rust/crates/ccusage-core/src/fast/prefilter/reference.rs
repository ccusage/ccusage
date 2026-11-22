use memchr::{memchr, memmem::Finder};
use smallvec::SmallVec;

#[derive(Clone, Copy)]
enum PrefilterMode {
    All,
    Any,
}

pub(super) struct LinePrefilter {
    markers: SmallVec<[Marker; 4]>,
    escapable: u128,
    short_escapable: bool,
    mode: PrefilterMode,
}

impl LinePrefilter {
    pub(super) fn all(markers: &[&[u8]]) -> Self {
        Self::new(markers, PrefilterMode::All)
    }

    pub(super) fn any(markers: &[&[u8]]) -> Self {
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

    pub(super) fn matches(&self, line: &[u8]) -> bool {
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

    pub(super) fn matches_escaped(&self, line: &[u8]) -> bool {
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

const SHORT_ESCAPE_BYTES: [u8; 8] = [b'"', b'\\', b'/', 0x08, 0x0c, b'\n', b'\r', b'\t'];

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
