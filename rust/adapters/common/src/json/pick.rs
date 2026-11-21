use std::borrow::Cow;

use memchr::memchr2;
use serde::{
    Deserialize, Deserializer,
    de::{
        DeserializeSeed, MapAccess, Visitor,
        value::{BorrowedStrDeserializer, StringDeserializer},
    },
    forward_to_deserialize_any,
};

/// The members of one JSON object that a record type reads.
pub type Fields = &'static [(&'static str, Member)];

/// How a picked member's value reaches the record type.
#[derive(Clone, Copy)]
pub enum Member {
    /// The value's text, parsed by `serde_json` as it is.
    Value,
    /// An object reduced to these members; any other value is kept as [`Member::Value`].
    /// Only a member the record types read as a struct may be reduced.
    Object(Fields),
}

/// The members a [`Fields`] list picked from one line, ready to deserialize.
pub struct Picked<'a> {
    members: Vec<Node<'a>>,
}

struct Node<'a> {
    key: Cow<'a, str>,
    value: Slot<'a>,
}

enum Slot<'a> {
    Raw(&'a str),
    Object(Vec<Node<'a>>),
}

// NOTE: A line nested deeper than this goes to the full parse, so neither path reaches the 128-level limit `serde_json` puts on typed values.
const MAX_DEPTH: usize = 64;

impl<'a> Picked<'a> {
    /// Reads `text` as one JSON object and keeps only the members in `fields`.
    ///
    /// `None` means the line needs the full parse: it is not a valid JSON object, a key fails to
    /// decode, it nests too deeply, or it holds a raw control byte. A `Some` deserializes to what
    /// `serde_json::from_str` gives for any derived struct whose fields `fields` lists.
    pub fn new(text: &'a str, fields: Fields) -> Option<Self> {
        let body = text.trim_matches(is_whitespace_char);
        if has_control_byte(body.as_bytes()) {
            return None;
        }
        let mut scan = Scan {
            text: body,
            bytes: body.as_bytes(),
            at: 0,
        };
        if scan.bytes.first() != Some(&b'{') {
            return None;
        }
        let members = scan.object(fields, 1)?;
        (scan.at == scan.bytes.len()).then_some(Self { members })
    }

    pub fn parse<T: Deserialize<'a>>(&self) -> serde_json::Result<T> {
        T::deserialize(ObjectDeserializer(&self.members))
    }
}

fn is_whitespace_char(char: char) -> bool {
    matches!(char, ' ' | '\n' | '\t' | '\r')
}

fn has_control_byte(bytes: &[u8]) -> bool {
    bytes.chunks(64).any(|chunk| {
        chunk
            .iter()
            .fold(false, |found, &byte| found | (byte < 0x20))
    })
}

struct Scan<'a> {
    text: &'a str,
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Scan<'a> {
    fn peek(&mut self) -> Option<u8> {
        while let Some(b' ' | b'\n' | b'\t' | b'\r') = self.bytes.get(self.at) {
            self.at += 1;
        }
        self.bytes.get(self.at).copied()
    }

    fn eat(&mut self, byte: u8) -> Option<()> {
        (self.peek()? == byte).then(|| self.at += 1)
    }

    /// Skips a string whose opening quote is already eaten; returns whether it holds an escape.
    fn string(&mut self) -> Option<bool> {
        let mut escaped = false;
        loop {
            let found = memchr2(b'"', b'\\', self.bytes.get(self.at..)?)?;
            self.at += found + 1;
            if self.bytes[self.at - 1] == b'"' {
                return Some(escaped);
            }
            escaped = true;
            match *self.bytes.get(self.at)? {
                b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => self.at += 1,
                b'u' => {
                    let digits = self.bytes.get(self.at + 1..self.at + 5)?;
                    if !digits.iter().all(u8::is_ascii_hexdigit) {
                        return None;
                    }
                    self.at += 5;
                }
                _ => return None,
            }
        }
    }

    fn key(&mut self) -> Option<Cow<'a, str>> {
        self.eat(b'"')?;
        let start = self.at;
        let escaped = self.string()?;
        let key = if escaped {
            Cow::Owned(serde_json::from_str::<String>(&self.text[start - 1..self.at]).ok()?)
        } else {
            Cow::Borrowed(&self.text[start..self.at - 1])
        };
        self.eat(b':')?;
        Some(key)
    }

    fn object(&mut self, fields: Fields, depth: usize) -> Option<Vec<Node<'a>>> {
        self.eat(b'{')?;
        let mut members = Vec::new();
        if self.peek()? == b'}' {
            self.at += 1;
            return Some(members);
        }
        loop {
            let key = self.key()?;
            match fields.iter().find(|(name, _)| *name == key) {
                Some((_, Member::Object(inner))) if self.peek()? == b'{' => {
                    let inner = self.object(inner, depth + 1)?;
                    members.push(Node {
                        key,
                        value: Slot::Object(inner),
                    });
                }
                Some(_) => {
                    let start = self.at;
                    self.value(depth)?;
                    members.push(Node {
                        key,
                        value: Slot::Raw(&self.text[start..self.at]),
                    });
                }
                None => self.value(depth)?,
            }
            match self.peek()? {
                b',' => self.at += 1,
                b'}' => {
                    self.at += 1;
                    return Some(members);
                }
                _ => return None,
            }
        }
    }

    fn value(&mut self, depth: usize) -> Option<()> {
        match self.peek()? {
            b'"' => {
                self.at += 1;
                self.string().map(drop)
            }
            b'{' => {
                let depth = nested(depth)?;
                self.at += 1;
                if self.peek()? == b'}' {
                    self.at += 1;
                    return Some(());
                }
                loop {
                    self.eat(b'"')?;
                    self.string()?;
                    self.eat(b':')?;
                    self.value(depth)?;
                    match self.peek()? {
                        b',' => self.at += 1,
                        b'}' => {
                            self.at += 1;
                            return Some(());
                        }
                        _ => return None,
                    }
                }
            }
            b'[' => {
                let depth = nested(depth)?;
                self.at += 1;
                if self.peek()? == b']' {
                    self.at += 1;
                    return Some(());
                }
                loop {
                    self.value(depth)?;
                    match self.peek()? {
                        b',' => self.at += 1,
                        b']' => {
                            self.at += 1;
                            return Some(());
                        }
                        _ => return None,
                    }
                }
            }
            b't' => self.literal(b"true"),
            b'f' => self.literal(b"false"),
            b'n' => self.literal(b"null"),
            b'-' | b'0'..=b'9' => self.number(),
            _ => None,
        }
    }

    fn literal(&mut self, literal: &[u8]) -> Option<()> {
        self.bytes[self.at..]
            .starts_with(literal)
            .then(|| self.at += literal.len())
    }

    fn digits(&mut self) -> usize {
        let count = self.bytes[self.at..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        self.at += count;
        count
    }

    fn number(&mut self) -> Option<()> {
        if self.bytes[self.at] == b'-' {
            self.at += 1;
        }
        match self.bytes.get(self.at)? {
            b'0' => {
                self.at += 1;
                if self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
                    return None;
                }
            }
            b'1'..=b'9' => {
                self.digits();
            }
            _ => return None,
        }
        if self.bytes.get(self.at) == Some(&b'.') {
            self.at += 1;
            if self.digits() == 0 {
                return None;
            }
        }
        if let Some(b'e' | b'E') = self.bytes.get(self.at) {
            self.at += 1;
            if let Some(b'+' | b'-') = self.bytes.get(self.at) {
                self.at += 1;
            }
            if self.digits() == 0 {
                return None;
            }
        }
        Some(())
    }
}

fn nested(depth: usize) -> Option<usize> {
    (depth < MAX_DEPTH).then_some(depth + 1)
}

struct ObjectDeserializer<'p, 'a>(&'p [Node<'a>]);

impl<'de> Deserializer<'de> for ObjectDeserializer<'_, 'de> {
    type Error = serde_json::Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> serde_json::Result<V::Value> {
        visitor.visit_map(Members {
            rest: self.0,
            value: None,
        })
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> serde_json::Result<V::Value> {
        visitor.visit_some(self)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> serde_json::Result<V::Value> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> serde_json::Result<V::Value> {
        visitor.visit_unit()
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf unit unit_struct seq tuple tuple_struct map struct enum identifier
    }
}

struct Members<'p, 'a> {
    rest: &'p [Node<'a>],
    value: Option<&'p Slot<'a>>,
}

impl<'a> MapAccess<'a> for Members<'_, 'a> {
    type Error = serde_json::Error;

    fn next_key_seed<K: DeserializeSeed<'a>>(
        &mut self,
        seed: K,
    ) -> serde_json::Result<Option<K::Value>> {
        let Some((node, rest)) = self.rest.split_first() else {
            return Ok(None);
        };
        self.rest = rest;
        self.value = Some(&node.value);
        match &node.key {
            Cow::Borrowed(key) => seed.deserialize(BorrowedStrDeserializer::new(key)),
            Cow::Owned(key) => seed.deserialize(StringDeserializer::new(key.clone())),
        }
        .map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'a>>(&mut self, seed: V) -> serde_json::Result<V::Value> {
        match self.value.take() {
            Some(Slot::Raw(text)) => {
                let mut deserializer = serde_json::Deserializer::from_str(text);
                let value = seed.deserialize(&mut deserializer)?;
                deserializer.end()?;
                Ok(value)
            }
            Some(Slot::Object(members)) => seed.deserialize(ObjectDeserializer(members)),
            None => Err(serde::de::Error::custom("value requested before its key")),
        }
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.rest.len())
    }
}

#[cfg(test)]
mod tests;
