use std::cell::OnceCell;

use serde::{Deserialize, de::DeserializeOwned};

mod pick;

pub use pick::{Fields, Member, Picked};

/// JSON bytes parsed through `serde_json`'s string reader, so each record type is compiled once.
///
/// Parsing equals `serde_json::from_slice` for targets that read strings as `str` or `String`.
/// Byte-reading targets (`deserialize_bytes` or `deserialize_byte_buf`, such as `&[u8]`, `CString`
/// or `serde_bytes`) must use `from_slice`: on invalid UTF-8 they would see the escapes that stand
/// in for the invalid bytes.
pub struct JsonText<'a> {
    bytes: &'a [u8],
    probe: OnceCell<Probe>,
    replaced: OnceCell<Box<str>>,
}

struct Probe {
    text: Box<str>,
    markers: Vec<Marker>,
}

struct Marker {
    line: usize,
    column: usize,
    string_start: usize,
}

const MARKER: &[u8] = br"\uDC00";

impl<'a> JsonText<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            probe: OnceCell::new(),
            replaced: OnceCell::new(),
        }
    }

    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// Equals `serde_json::from_slice` when `T` reads strings as `str` or `String`; see [`JsonText`].
    #[inline]
    pub fn parse<'b, T: Deserialize<'b>>(&'b self) -> serde_json::Result<T> {
        self.parse_with(serde_json::from_str)
    }

    /// Runs `parse` on the text until its result matches `serde_json::from_slice` on the bytes, for
    /// string-reading targets only; see [`JsonText`].
    #[inline]
    pub fn parse_with<'b, R>(
        &'b self,
        mut parse: impl FnMut(&'b str) -> serde_json::Result<R>,
    ) -> serde_json::Result<R> {
        if let Ok(text) = simdutf8::basic::from_utf8(self.bytes) {
            return parse(text);
        }
        let result = parse(self.probe_text());
        let Err(error) = &result else {
            return result;
        };
        match self.marked_string_error(error) {
            Some(error) => Err(error),
            None => parse(self.replaced_text()),
        }
    }

    /// Keeps only `fields` of a valid UTF-8 line; see [`Picked::new`]. `None` means the line
    /// needs [`JsonText::parse`].
    #[inline]
    pub fn pick(&self, fields: Fields) -> Option<Picked<'a>> {
        Picked::new(simdutf8::basic::from_utf8(self.bytes).ok()?, fields)
    }

    #[cold]
    #[inline(never)]
    fn probe_text(&self) -> &str {
        &self.probe.get_or_init(|| Probe::new(self.bytes)).text
    }

    #[cold]
    #[inline(never)]
    fn replaced_text(&self) -> &str {
        self.replaced.get_or_init(|| replaced(self.bytes))
    }

    #[cold]
    #[inline(never)]
    fn marked_string_error(&self, error: &serde_json::Error) -> Option<serde_json::Error> {
        let probe = self.probe.get()?;
        let marker = probe.markers.iter().find(|marker| {
            error.classify() == serde_json::error::Category::Syntax
                && marker.line == error.line()
                && marker.column == error.column()
        })?;
        string_error(self.bytes, marker.string_start)
    }
}

/// Parses owned values with [`JsonText::parse`], under the same string-target contract.
pub fn parse<T: DeserializeOwned>(bytes: &[u8]) -> serde_json::Result<T> {
    JsonText::new(bytes).parse()
}

impl Probe {
    // NOTE(2026-11-20): serde_json 1.0.154 checks UTF-8 only in strings it parses, and skips ignored strings and their lone `\uDC00` escapes unchecked. Marking each invalid in-string byte with that escape reproduces which records decode, and a parsed marker shows where the slice reader would fail.
    fn new(bytes: &[u8]) -> Self {
        enum State {
            Outside,
            String { after_lead: bool },
            Escape,
            Hex { left: u8, value: u16 },
        }
        let invalid = invalid_bytes(bytes);
        let mut text = Vec::with_capacity(bytes.len() + MARKER.len());
        let mut markers = Vec::new();
        let (mut line, mut line_start, mut string_start) = (1, 0, 0);
        let mut state = State::Outside;
        for (index, (&byte, &bad)) in bytes.iter().zip(&invalid).enumerate() {
            state = match state {
                State::String { after_lead: false } if bad => {
                    text.extend_from_slice(MARKER);
                    markers.push(Marker {
                        line,
                        column: text.len() - line_start,
                        string_start,
                    });
                    text.push(b'_');
                    State::String { after_lead: false }
                }
                _ if bad => {
                    text.push(b'?');
                    match state {
                        State::Hex { left, value } => hex(left, value, 0),
                        State::Outside => State::Outside,
                        State::String { .. } | State::Escape => State::String { after_lead: false },
                    }
                }
                State::Outside => {
                    text.push(byte);
                    if byte == b'"' {
                        string_start = index;
                        State::String { after_lead: false }
                    } else {
                        State::Outside
                    }
                }
                State::String { .. } => {
                    text.push(byte);
                    match byte {
                        b'"' => State::Outside,
                        b'\\' => State::Escape,
                        _ => State::String { after_lead: false },
                    }
                }
                State::Escape => {
                    text.push(byte);
                    if byte == b'u' {
                        State::Hex { left: 4, value: 0 }
                    } else {
                        State::String { after_lead: false }
                    }
                }
                State::Hex { left, value } => {
                    text.push(byte);
                    hex(left, value, (byte as char).to_digit(16).unwrap_or(0) as u16)
                }
            };
            if byte == b'\n' {
                line += 1;
                line_start = text.len();
            }
        }
        fn hex(left: u8, value: u16, digit: u16) -> State {
            let value = value.wrapping_mul(16).wrapping_add(digit);
            if left > 1 {
                State::Hex {
                    left: left - 1,
                    value,
                }
            } else {
                State::String {
                    after_lead: (0xD800..=0xDBFF).contains(&value),
                }
            }
        }
        Self {
            text: String::from_utf8(text)
                .expect("probe text replaces every invalid byte")
                .into_boxed_str(),
            markers,
        }
    }
}

fn invalid_bytes(bytes: &[u8]) -> Vec<bool> {
    let mut invalid = vec![false; bytes.len()];
    let mut start = 0;
    while let Err(error) = std::str::from_utf8(&bytes[start..]) {
        let bad = start + error.valid_up_to();
        let len = error.error_len().unwrap_or(bytes.len() - bad);
        invalid[bad..bad + len].fill(true);
        start = bad + len;
    }
    invalid
}

fn replaced(bytes: &[u8]) -> Box<str> {
    let text = bytes
        .iter()
        .zip(invalid_bytes(bytes))
        .map(|(&byte, bad)| if bad { b'?' } else { byte })
        .collect();
    String::from_utf8(text)
        .expect("every invalid byte is replaced")
        .into_boxed_str()
}

fn string_error(bytes: &[u8], string_start: usize) -> Option<serde_json::Error> {
    let mut padded = bytes.to_vec();
    for byte in &mut padded[..string_start] {
        if *byte != b'\n' {
            *byte = b' ';
        }
    }
    serde_json::from_slice::<&str>(&padded).err()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde::{Deserialize, de::IgnoredAny};

    use super::JsonText;
    use crate::jsonl::{lenient_object, non_empty_string};

    #[derive(Debug, Deserialize, PartialEq)]
    struct Borrowed<'a> {
        tokens: Option<u64>,
        #[serde(borrow)]
        model: Option<&'a str>,
        #[serde(borrow)]
        tags: Option<Vec<&'a str>>,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Owned {
        #[serde(default, deserialize_with = "non_empty_string")]
        model: Option<String>,
        #[serde(default, deserialize_with = "lenient_object")]
        usage: Option<Usage>,
        note: Option<IgnoredAny>,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Usage {
        input: Option<u64>,
        name: Option<String>,
    }

    fn same<'a, T>(bytes: &'a [u8], text: &'a JsonText<'a>)
    where
        T: Deserialize<'a> + PartialEq + std::fmt::Debug,
    {
        let expected = serde_json::from_slice::<T>(bytes);
        let actual = text.parse::<T>();
        match (expected, actual) {
            (Ok(expected), Ok(actual)) => {
                assert_eq!(expected, actual, "{:?}", bytes.escape_ascii().to_string())
            }
            (Err(expected), Err(actual)) => {
                assert_eq!(
                    expected.to_string(),
                    actual.to_string(),
                    "{:?}",
                    bytes.escape_ascii().to_string()
                );
                assert_eq!(expected.classify(), actual.classify());
            }
            (expected, actual) => panic!(
                "{:?}: slice reader {:?}, text {:?}",
                bytes.escape_ascii().to_string(),
                expected.map(|_| ()),
                actual.map(|_| ())
            ),
        }
    }

    fn check(bytes: &[u8]) {
        same::<Borrowed<'_>>(bytes, &JsonText::new(bytes));
        same::<Owned>(bytes, &JsonText::new(bytes));
        same::<serde_json::Value>(bytes, &JsonText::new(bytes));
        same::<BTreeMap<String, IgnoredAny>>(bytes, &JsonText::new(bytes));
        same::<Option<IgnoredAny>>(bytes, &JsonText::new(bytes));
    }

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, bound: usize) -> usize {
            (self.next() % bound as u64) as usize
        }

        fn pick<'a>(&mut self, items: &[&'a [u8]]) -> &'a [u8] {
            items[self.below(items.len())]
        }
    }

    const STRING_PIECES: &[&[u8]] = &[
        b"a",
        b"model",
        b"\xc3\xa9",
        b"\xe2\x80\x94",
        b"\xf0\x9f\x98\x80",
        br"\n",
        br#"\""#,
        br"\\",
        br"\u0041",
        br"\uD83D\uDE00",
        br"\uD800",
        br"\uDC00",
        br"\u12",
        br"\x",
        b"\xff",
        b"\x80",
        b"\xe3\x81",
        b"\xc0\xaf",
        b"\x01",
        b" ",
    ];

    const KEYS: &[&[u8]] = &[
        b"tokens",
        b"model",
        b"tags",
        b"usage",
        b"input",
        b"name",
        b"note",
        b"parts",
        b"items",
        b"content",
        b"\xffkey",
        b"mod\x80el",
    ];

    fn string(rng: &mut Rng, out: &mut Vec<u8>) {
        out.push(b'"');
        for _ in 0..rng.below(5) {
            out.extend_from_slice(rng.pick(STRING_PIECES));
        }
        out.push(b'"');
    }

    fn value(rng: &mut Rng, depth: usize, out: &mut Vec<u8>) {
        match rng.below(if depth > 3 { 4 } else { 7 }) {
            0 => string(rng, out),
            1 => out.extend_from_slice(rng.pick(&[
                b"12",
                b"-3",
                b"4.5e1",
                b"0",
                b"18446744073709551615",
            ])),
            2 => out.extend_from_slice(rng.pick(&[b"true", b"false", b"null"])),
            3 => string(rng, out),
            4 | 5 => {
                out.push(b'{');
                for index in 0..rng.below(5) {
                    if index > 0 {
                        out.push(b',');
                    }
                    if rng.below(3) == 0 {
                        out.push(b'\n');
                    }
                    out.push(b'"');
                    out.extend_from_slice(rng.pick(KEYS));
                    out.extend_from_slice(b"\":");
                    value(rng, depth + 1, out);
                }
                out.push(b'}');
            }
            _ => {
                out.push(b'[');
                for index in 0..rng.below(4) {
                    if index > 0 {
                        out.push(b',');
                    }
                    value(rng, depth + 1, out);
                }
                out.push(b']');
            }
        }
    }

    fn corrupt(rng: &mut Rng, line: &mut Vec<u8>) {
        for _ in 0..rng.below(4) {
            if line.is_empty() {
                return;
            }
            let at = rng.below(line.len() + 1);
            match rng.below(6) {
                0 => line.insert(at, [0xff, 0x80, 0xc3, 0xe3, 0xf0][rng.below(5)]),
                1 => line.truncate(at),
                2 if at < line.len() => {
                    line.remove(at);
                }
                3 => line.insert(at, b"\",:{}[]\\ "[rng.below(9)]),
                _ => {}
            }
        }
    }

    #[test]
    fn matches_the_slice_reader_on_generated_lines() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for _ in 0..200_000 {
            let mut line = Vec::new();
            line.push(b'{');
            for index in 0..rng.below(6) {
                if index > 0 {
                    line.push(b',');
                }
                line.push(b'"');
                line.extend_from_slice(rng.pick(KEYS));
                line.extend_from_slice(b"\":");
                value(&mut rng, 0, &mut line);
            }
            line.push(b'}');
            corrupt(&mut rng, &mut line);
            check(&line);
        }
    }

    #[test]
    fn invalid_bytes_in_ignored_fields_still_decode() {
        let bytes = b"{\"tokens\":1,\"note\":\"\xff\"}";
        assert_eq!(
            JsonText::new(bytes).parse::<Borrowed<'_>>().unwrap(),
            Borrowed {
                tokens: Some(1),
                model: None,
                tags: None,
            }
        );
    }

    #[test]
    fn invalid_bytes_in_parsed_fields_fail_where_the_slice_reader_does() {
        let bytes = b"{\"tokens\":3,\"note\":\"\xff\",\"model\":\"a\\n\xffb\"}";
        assert_eq!(
            JsonText::new(bytes)
                .parse::<Borrowed<'_>>()
                .unwrap_err()
                .to_string(),
            serde_json::from_slice::<Borrowed<'_>>(bytes)
                .unwrap_err()
                .to_string(),
        );
    }

    #[derive(Debug, PartialEq)]
    struct RawBytes(Vec<u8>);

    impl<'de> Deserialize<'de> for RawBytes {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct BytesVisitor;

            impl serde::de::Visitor<'_> for BytesVisitor {
                type Value = RawBytes;

                fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    formatter.write_str("a byte string")
                }

                fn visit_bytes<E>(self, bytes: &[u8]) -> Result<RawBytes, E> {
                    Ok(RawBytes(bytes.to_vec()))
                }
            }

            #[expect(
                clippy::disallowed_methods,
                reason = "pins the byte-target divergence JsonText documents"
            )]
            deserializer.deserialize_bytes(BytesVisitor)
        }
    }

    #[test]
    fn byte_targets_see_probe_escapes_instead_of_invalid_bytes() {
        let bytes = b"\"a\xffb\"";
        assert_eq!(
            serde_json::from_slice::<RawBytes>(bytes).unwrap(),
            RawBytes(b"a\xffb".to_vec())
        );
        assert_eq!(
            JsonText::new(bytes).parse::<RawBytes>().unwrap(),
            RawBytes(b"a\xed\xb0\x80_b".to_vec())
        );
    }
}
