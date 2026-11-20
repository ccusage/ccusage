use std::{fmt, sync::LazyLock};

use memchr::memmem::FinderRev;

use serde::{
    Deserialize, Deserializer,
    de::{IgnoredAny, MapAccess, SeqAccess, Visitor},
};

use super::super::response_chain::{Links, key};

#[derive(Default)]
struct Uuid(Option<u64>);

struct UuidVisitor;

impl<'de> Visitor<'de> for UuidVisitor {
    type Value = Uuid;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any JSON value")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(Uuid(Some(key(value.as_bytes()))))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Uuid(None))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(Uuid(None))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(Uuid(None))
    }

    fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
        Ok(Uuid(None))
    }

    fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
        Ok(Uuid(None))
    }

    fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
        Ok(Uuid(None))
    }

    fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
        Ok(Uuid(None))
    }
}

impl<'de> Deserialize<'de> for Uuid {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(UuidVisitor)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LinkRecord {
    #[serde(default)]
    uuid: Uuid,
    #[serde(default)]
    parent_uuid: Uuid,
}

// NOTE(2026-11-20, #2255): Decoding these links with serde costs a second full parse of every requestless usage line. Claude Code writes `parentUuid` first and `uuid` among the trailing scalar keys, and a JSON string cannot hold those byte sequences unescaped, so anchored searches find them. Other layouts take the serde parse.
pub(in crate::daily) fn parse_links(line: &[u8]) -> Option<Links> {
    static UUID: LazyLock<FinderRev<'static>> = LazyLock::new(|| FinderRev::new(br#","uuid":""#));
    let Some(rest) = line.strip_prefix(br#"{"parentUuid":"#) else {
        return parse_links_slowly(line);
    };
    let parent = match rest.first()? {
        b'"' => Some(key(string_at(&rest[1..])?)),
        b'n' if rest.starts_with(b"null") => None,
        _ => return parse_links_slowly(line),
    };
    let uuid = UUID
        .rfind(line)
        .and_then(|at| string_at(&line[at + UUID.needle().len()..]))
        .map(key);
    Some(Links { uuid, parent })
}

fn string_at(bytes: &[u8]) -> Option<&[u8]> {
    let end = memchr::memchr2(b'"', b'\\', bytes)?;
    (bytes[end] == b'"').then(|| &bytes[..end])
}

fn parse_links_slowly(line: &[u8]) -> Option<Links> {
    let record = crate::deserialize_usage_line::<LinkRecord>(line).ok()?;
    Some(Links {
        uuid: record.uuid.0,
        parent: record.parent_uuid.0,
    })
}
