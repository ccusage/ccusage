use std::{fs::File, io::Read, ops::ControlFlow, path::Path};

use ccusage_adapter_common::UnitError;
use ccusage_core::{Diagnostics, Skip, SkipKind, fast::LineReader};

const READ_BUFFER_SIZE: usize = 256 * 1024;

/// One line of a transcript, without its `\n`.
#[derive(Clone, Copy)]
pub(super) struct Line<'a> {
    pub(super) index: usize,
    pub(super) bytes: &'a [u8],
    /// The file ends inside this line, so its record may still be being written.
    unterminated: bool,
}

/// Visits every line of `path` in order.
///
/// A read failure still visits the bytes received before it as a final line,
/// so a valid unterminated record survives an interrupted read.
pub(super) fn for_each_line(path: &Path, diagnostics: &Diagnostics, visit: impl FnMut(Line<'_>)) {
    let Ok(file) = File::open(path) else {
        UnitError::new(SkipKind::Unreadable, "file open failed").record(path, diagnostics);
        return;
    };
    for_each_line_from(file, path, diagnostics, visit);
}

fn for_each_line_from(
    reader: impl Read,
    path: &Path,
    diagnostics: &Diagnostics,
    mut visit: impl FnMut(Line<'_>),
) {
    let read = visit_lines(reader, |line| {
        visit(line);
        ControlFlow::Continue(())
    });
    if read.interrupted {
        diagnostics.record(Skip::unit(
            path,
            SkipKind::ReadInterrupted,
            "file read failed",
        ));
    }
}

pub(super) struct LinesRead {
    pub(super) stopped: bool,
    interrupted: bool,
}

pub(super) fn visit_lines(
    reader: impl Read,
    mut visit: impl FnMut(Line<'_>) -> ControlFlow<()>,
) -> LinesRead {
    let end = |flow: ControlFlow<()>, interrupted| LinesRead {
        stopped: flow.is_break(),
        interrupted,
    };
    let mut lines = LineReader::with_capacity(READ_BUFFER_SIZE, reader);
    let mut index = 0;
    loop {
        match lines.next_line() {
            Ok(Some(line)) => {
                let (bytes, unterminated) = match line.strip_suffix(b"\n") {
                    Some(bytes) => (bytes, false),
                    None => (line, true),
                };
                let flow = visit(Line {
                    index,
                    bytes,
                    unterminated,
                });
                if flow.is_break() {
                    return end(flow, false);
                }
            }
            Ok(None) => return end(ControlFlow::Continue(()), false),
            Err(_) => {
                let flow = if lines.rest().is_empty() {
                    ControlFlow::Continue(())
                } else {
                    visit(Line {
                        index,
                        bytes: lines.rest(),
                        unterminated: true,
                    })
                };
                return end(flow, true);
            }
        }
        index += 1;
    }
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

/// Records why `line` of `path` was not counted.
#[cold]
pub(super) fn rejected(path: &Path, line: Line<'_>, diagnostics: &Diagnostics) {
    let number = Skip::line_number(line.index);
    let Some(value) = ccusage_adapter_common::jsonl::decode_streamed_line::<serde_json::Value>(
        line.bytes,
        line.unterminated,
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
    let wrapped = value
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

        let mut visited = false;
        for_each_line(&fixture.path("missing.jsonl"), &diagnostics, |_| {
            visited = true;
        });
        assert!(!visited);

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
            terminated(0, line),
            &diagnostics,
        );

        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
    }

    fn terminated(index: usize, bytes: &[u8]) -> Line<'_> {
        Line {
            index,
            bytes,
            unterminated: false,
        }
    }

    fn read_lines(reader: impl Read, diagnostics: &Diagnostics) -> Vec<(usize, Vec<u8>, bool)> {
        let mut lines = Vec::new();
        for_each_line_from(reader, Path::new("session.jsonl"), diagnostics, |line| {
            lines.push((line.index, line.bytes.to_vec(), line.unterminated));
        });
        lines
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
        let lines = read_lines(Cursor::new(line).chain(Interrupted), &diagnostics);
        assert_eq!(lines.len(), 1);
        let entry = crate::daily::parse_usage_line(&lines[0].1).unwrap().0;
        assert_eq!(entry.message.usage.usage.input_tokens, 10);
        assert_eq!(entry.message.usage.usage.output_tokens, 2);
        assert_eq!(diagnostics.skipped(SkipKind::ReadInterrupted), 1);
    }

    #[test]
    fn preserves_received_bytes_before_a_read_interruption() {
        let diagnostics = Diagnostics::new("claude");
        let lines = read_lines(
            Cursor::new(b"complete\npartial").chain(Interrupted),
            &diagnostics,
        );
        assert_eq!(
            lines,
            [
                (0, b"complete".to_vec(), false),
                (1, b"partial".to_vec(), true)
            ]
        );
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
    const CUT_LAST_LINE: &str = r#"{"message":{"usage":{"input_tokens":10}}}
{"message":{"usage":{"input_tok"#;

    /// Streams `path` and rejects its unterminated last line the way the
    /// loaders do.
    fn reject_last_line(path: &Path, diagnostics: &Diagnostics) {
        let mut rejected_lines = 0;
        for_each_line(path, diagnostics, |line| {
            if line.unterminated {
                rejected_lines += 1;
                rejected(path, line, diagnostics);
            }
        });
        assert_eq!(rejected_lines, 1);
    }

    #[test]
    fn a_usage_line_still_being_written_is_not_a_skip() {
        let diagnostics = Diagnostics::new("claude");
        let fixture = ccusage_test_support::Fixture::new();
        let path = fixture.write_file("session.jsonl", CUT_LAST_LINE);

        reject_last_line(&path, &diagnostics);

        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 0);
    }

    #[test]
    fn a_usage_line_left_cut_short_is_skipped_as_truncated() {
        let diagnostics = Diagnostics::new("claude");
        let fixture = ccusage_test_support::Fixture::new();
        let path = fixture.write_file("session.jsonl", CUT_LAST_LINE);
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(120))
            .unwrap();

        reject_last_line(&path, &diagnostics);

        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
    }

    #[test]
    fn unrelated_and_intentionally_unsupported_records_stay_quiet() {
        let diagnostics = Diagnostics::new("claude");
        for line in [
            br#"{"metadata":{"usage":{}}}"#.as_slice(),
            br#"{"data":{"usage":{},"content":[{"text":"\uD83D\uDE00"},true,false,null,-1,2,3.5]}}"#,
            br#"{"m\u0065ssage":null,"data":{"usage":{}}}"#,
        ] {
            rejected(
                Path::new("session.jsonl"),
                terminated(0, line),
                &diagnostics,
            );
        }
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 0);
        assert_eq!(diagnostics.skipped(SkipKind::UndecodedUsage), 0);
    }

    #[test]
    fn a_rejected_wrapped_usage_record_is_invalid_data() {
        let diagnostics = Diagnostics::new("claude");
        rejected(
            Path::new("session.jsonl"),
            terminated(0, br#"{"data":{"message":{"message":{"usage":{}}}}}"#),
            &diagnostics,
        );
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
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
            rejected(
                Path::new("session.jsonl"),
                terminated(0, line),
                &diagnostics,
            );
            assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        }
    }
}
