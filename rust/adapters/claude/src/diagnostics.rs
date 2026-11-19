use std::{fs::File, io::Read, path::Path};

use ccusage_adapter_common::UnitError;
use ccusage_core::{Diagnostics, Skip, SkipKind};

pub(super) fn read_content(path: &Path, diagnostics: &Diagnostics) -> Vec<u8> {
    let Ok(mut file) = File::open(path) else {
        UnitError::new(SkipKind::Unreadable, "file open failed").record(path, diagnostics);
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
    }
    content
}

/// Records that line `index` of `path` carried a timestamp no day holds.
pub(super) fn invalid_timestamp(path: &Path, index: usize, diagnostics: &Diagnostics) {
    diagnostics.record(Skip::record(
        path,
        Skip::line_number(index),
        SkipKind::InvalidTimestamp,
        "invalid usage timestamp",
    ));
}

/// Records why `line`, line `index` of `content`, was not counted.
pub(super) fn rejected(
    path: &Path,
    content: &[u8],
    index: usize,
    line: &[u8],
    diagnostics: &Diagnostics,
    progress: bool,
) {
    let number = Skip::line_number(index);
    let Some(value) = ccusage_adapter_common::jsonl::decode_line::<serde_json::Value>(
        content,
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
        "usage record was rejected",
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor};

    #[test]
    fn a_file_removed_after_discovery_is_not_a_skip() {
        let fixture = ccusage_test_support::Fixture::new();
        let diagnostics = Diagnostics::new("claude");

        assert!(read_content(&fixture.path("missing.jsonl"), &diagnostics).is_empty());

        assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 0);
        assert!(
            diagnostics
                .warnings(&ccusage_core::cli::SharedArgs::default())
                .is_empty()
        );
    }

    #[test]
    fn malformed_usage_is_diagnosed() {
        let diagnostics = Diagnostics::new("claude");

        let line = br#"{"message":{"usage":{"SECRET_CANARY":"#;
        rejected(
            Path::new("session.jsonl"),
            &[line.as_slice(), b"\n"].concat(),
            0,
            line,
            &diagnostics,
            false,
        );

        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
    }

    struct Interrupted;
    impl Read for Interrupted {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("SECRET_CANARY"))
        }
    }
    #[test]
    fn retains_valid_unterminated_usage_before_a_read_interruption() {
        let diagnostics = Diagnostics::new("claude");
        let line = br#"{"timestamp":"2026-09-01T00:00:01Z","message":{"model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":2}}}"#;
        let mut reader = Cursor::new(line).chain(Interrupted);
        let bytes = read_content_from(&mut reader, 0, Path::new("session.jsonl"), &diagnostics);
        let entry = crate::deserialize_usage_line::<crate::UsageEntry>(&bytes).unwrap();
        assert_eq!(entry.message.usage.input_tokens, 10);
        assert_eq!(entry.message.usage.output_tokens, 2);
        assert_eq!(diagnostics.skipped(SkipKind::ReadInterrupted), 1);
    }

    #[test]
    fn preserves_received_bytes_before_a_read_interruption() {
        let diagnostics = Diagnostics::new("claude");
        let mut reader = Cursor::new(b"complete\npartial").chain(Interrupted);
        let bytes = read_content_from(&mut reader, 0, Path::new("session.jsonl"), &diagnostics);
        assert_eq!(bytes, b"complete\npartial");
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
    fn a_usage_line_still_being_written_is_not_a_skip() {
        let diagnostics = Diagnostics::new("claude");
        let text = r#"{"message":{"usage":{"input_tokens":10}}}
{"message":{"usage":{"input_tok"#;
        let fixture = ccusage_test_support::Fixture::new();
        let path = fixture.write_file("session.jsonl", text);
        let content = text.as_bytes();
        let line = &content[content.iter().position(|byte| *byte == b'\n').unwrap() + 1..];

        rejected(&path, content, 1, line, &diagnostics, false);

        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 0);
    }

    #[test]
    fn unrelated_and_intentionally_unsupported_records_stay_quiet() {
        let diagnostics = Diagnostics::new("claude");
        for line in [
            br#"{"metadata":{"usage":{}}}"#.as_slice(),
            br#"{"data":{"message":{"message":{"usage":{}}}}}"#,
        ] {
            rejected(
                Path::new("session.jsonl"),
                line,
                0,
                line,
                &diagnostics,
                false,
            );
        }
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 0);
        assert_eq!(diagnostics.skipped(SkipKind::UndecodedUsage), 0);
    }
}
