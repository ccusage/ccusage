use std::{fs::File, io::Read, path::Path};

use ccusage_core::{Diagnostics, Skip, SkipKind};
use serde::{Deserialize, de::IgnoredAny};

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

#[derive(Deserialize)]
struct DirectEnvelope {
    message: Option<IgnoredAny>,
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
        ] {
            rejected(Path::new("session.jsonl"), 0, line, &diagnostics, false);
        }
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 0);
        assert_eq!(diagnostics.skipped(SkipKind::UndecodedUsage), 0);
    }
}
