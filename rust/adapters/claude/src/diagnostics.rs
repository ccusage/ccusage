use std::{fmt, fs::File, io::Read, path::Path};

use ccusage_core::{Diagnostics, Skip, SkipKind};
use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, SeqAccess, Visitor},
};

pub(super) fn emit(warnings: &[String]) {
    for warning in warnings {
        eprintln!("{warning}");
    }
}

pub(super) fn read_content(path: &Path, diagnostics: &Diagnostics) -> Vec<u8> {
    let Ok(mut file) = File::open(path) else {
        diagnostics.record(Skip::unit(path, SkipKind::Unreadable, "file open failed"));
        return Vec::new();
    };
    let capacity = file
        .metadata()
        .map_or(0, |metadata| metadata.len() as usize);
    read_content_from(&mut file, capacity, path, diagnostics)
}

fn read_content_from(
    reader: &mut impl Read,
    capacity: usize,
    path: &Path,
    diagnostics: &Diagnostics,
) -> Vec<u8> {
    let mut content = Vec::with_capacity(capacity);
    if reader.read_to_end(&mut content).is_err() {
        diagnostics.record(Skip::unit(
            path,
            SkipKind::ReadInterrupted,
            "file read failed",
        ));
        content.truncate(
            content
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(0, |index| index + 1),
        );
    }
    content
}

struct DirectEnvelope {
    message: Option<CheckedValue>,
}

impl<'de> Deserialize<'de> for DirectEnvelope {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EnvelopeVisitor;
        impl<'de> Visitor<'de> for EnvelopeVisitor {
            type Value = DirectEnvelope;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                #[derive(Deserialize)]
                #[serde(field_identifier)]
                enum Field {
                    #[serde(rename = "message")]
                    Message,
                    #[serde(other)]
                    Other,
                }

                let mut message = None;
                while let Some(field) = map.next_key::<Field>()? {
                    let value = map.next_value::<Option<CheckedValue>>()?;
                    if matches!(field, Field::Message) {
                        message = value;
                    }
                }
                Ok(DirectEnvelope { message })
            }
        }
        deserializer.deserialize_map(EnvelopeVisitor)
    }
}

// IgnoredAny skips string and number validation that Value decoding performs.
// Discard through deserialize_any so diagnostics retain those decoding failures.
struct CheckedValue;

impl<'de> Deserialize<'de> for CheckedValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(Self)
    }
}

impl<'de> Visitor<'de> for CheckedValue {
    type Value = Self;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self, E> {
        Ok(Self)
    }

    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self, E> {
        Ok(Self)
    }

    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self, E> {
        Ok(Self)
    }

    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self, E> {
        Ok(Self)
    }

    fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Self, E> {
        Ok(Self)
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Self, E> {
        Ok(Self)
    }

    fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<Self, S::Error> {
        while seq.next_element::<Self>()?.is_some() {}
        Ok(Self)
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self, M::Error> {
        while map.next_entry::<Self, Self>()?.is_some() {}
        Ok(Self)
    }
}

#[cold]
pub(super) fn rejected(
    path: &Path,
    index: usize,
    line: &[u8],
    diagnostics: &Diagnostics,
    progress: bool,
) {
    if !progress
        && serde_json::from_slice::<DirectEnvelope>(line)
            .is_ok_and(|envelope| envelope.message.is_none())
    {
        return;
    }
    let number = u32::try_from(index + 1).unwrap_or(u32::MAX);
    let Some(value) = ccusage_adapter_common::jsonl::decode_record::<serde_json::Value>(
        line,
        path,
        number,
        diagnostics,
        None,
    ) else {
        return;
    };
    let direct = value
        .get("message")
        .and_then(|message| message.get("usage"))
        .is_some_and(serde_json::Value::is_object);
    let wrapped = progress
        && value
            .pointer("/data/message/message/usage")
            .is_some_and(serde_json::Value::is_object);
    if !direct && !wrapped {
        return;
    }
    diagnostics.record(Skip::record(
        path,
        number,
        SkipKind::InvalidData,
        "usage record did not decode",
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor};

    #[test]
    fn missing_files_and_malformed_usage_are_diagnosed() {
        let fixture = ccusage_test_support::Fixture::new();
        let diagnostics = Diagnostics::new("claude");

        assert!(read_content(&fixture.path("missing.jsonl"), &diagnostics).is_empty());
        rejected(
            Path::new("session.jsonl"),
            0,
            br#"{"message":{"usage":{"SECRET_CANARY":"#,
            &diagnostics,
            false,
        );

        assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 1);
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
    }
    struct Interrupted;
    impl Read for Interrupted {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("SECRET_CANARY"))
        }
    }
    #[test]
    fn keeps_complete_lines_before_a_read_interruption() {
        let diagnostics = Diagnostics::new("claude");
        let mut reader = Cursor::new(b"complete\npartial").chain(Interrupted);
        let bytes = read_content_from(&mut reader, 0, Path::new("session.jsonl"), &diagnostics);
        assert_eq!(bytes, b"complete\n");
        assert_eq!(diagnostics.skipped(SkipKind::ReadInterrupted), 1);
        assert!(
            !diagnostics
                .warnings(&ccusage_core::cli::SharedArgs {
                    debug: true,
                    ..Default::default()
                })
                .join("\n")
                .contains("SECRET_CANARY")
        );
    }
    #[test]
    fn unrelated_and_intentionally_unsupported_records_stay_quiet() {
        let diagnostics = Diagnostics::new("claude");
        for line in [
            br#"{"metadata":{"usage":{}}}"#.as_slice(),
            br#"{"data":{"message":{"message":{"usage":{}}}}}"#,
            br#"{"data":{"usage":{},"content":[{"text":"\uD83D\uDE00"},true,false,null,-1,2,3.5]}}"#,
            br#"{"m\u0065ssage":null,"data":{"usage":{}}}"#,
        ] {
            rejected(Path::new("session.jsonl"), 0, line, &diagnostics, false);
        }
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 0);
        assert_eq!(diagnostics.skipped(SkipKind::UndecodedUsage), 0);
    }

    #[test]
    fn invalid_ignored_values_keep_decoding_diagnostics() {
        for line in [
            br#"{"data":{"message":{"message":{"usage":{},"content":"\uD800"}}}}"#.as_slice(),
            br#"{"data":{"usage":{},"content":"\uDC00"}}"#,
            b"{\"data\":{\"usage\":{},\"content\":\"\xff\"}}",
            br#"{"data":{"usage":{},"value":1e400}}"#,
        ] {
            let diagnostics = Diagnostics::new("claude");
            rejected(Path::new("session.jsonl"), 0, line, &diagnostics, false);
            assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        }
    }
}
