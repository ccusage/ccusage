use std::path::Path;

use ccusage_adapter_common::jsonl::Line;
use ccusage_core::{Diagnostics, Skip, SkipKind};

pub(super) fn read_failed(path: &Path, diagnostics: &Diagnostics) {
    diagnostics.record(Skip::unit(
        path,
        SkipKind::ReadInterrupted,
        "file read failed",
    ));
}

pub(super) fn invalid_timestamp(path: &Path, index: usize, diagnostics: &Diagnostics) {
    diagnostics.record(Skip::record(
        path,
        Skip::line_number(index),
        SkipKind::InvalidTimestamp,
        "invalid usage timestamp",
    ));
}

#[cold]
pub(super) fn rejected(path: &Path, line: Line<'_>, diagnostics: &Diagnostics) {
    let number = Skip::line_number(line.index);
    let Some(value) = line.record::<serde_json::Value>(path, diagnostics, None) else {
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
    use std::{
        fs::File,
        io::{self, Cursor, Read},
        ops::ControlFlow,
    };

    use ccusage_adapter_common::jsonl::{DEFAULT_BUFFER_SIZE, JsonLines, Tail, scan};
    use ccusage_core::{cli::CostMode, cost::PriceTable};

    #[test]
    fn a_file_removed_after_discovery_is_not_a_skip() {
        let fixture = ccusage_test_support::Fixture::new();
        let diagnostics = Diagnostics::new("claude");

        let pricing = ccusage_core::PricingMap::default();
        let entries = crate::daily::read_usage_file(
            &fixture.path("missing.jsonl"),
            &jiff::tz::TimeZone::UTC,
            PriceTable::for_map(CostMode::Auto, &pricing),
            &diagnostics,
            |_, _| (),
        );
        assert!(entries.is_empty());

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
        reject_terminated(line, &diagnostics);

        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
    }

    fn reject_terminated(bytes: &[u8], diagnostics: &Diagnostics) {
        let result = scan(
            bytes.chain(b"\n".as_slice()),
            DEFAULT_BUFFER_SIZE,
            Tail::Keep,
            |line| {
                rejected(Path::new("session.jsonl"), line, diagnostics);
                ControlFlow::<()>::Continue(())
            },
        );
        assert!(result.failure.is_none());
    }

    fn read_lines(reader: impl Read, diagnostics: &Diagnostics) -> Vec<(usize, Vec<u8>, bool)> {
        let mut lines = Vec::new();
        let result = scan(reader, DEFAULT_BUFFER_SIZE, Tail::Keep, |line| {
            lines.push((line.index, line.bytes().to_vec(), line.unterminated()));
            ControlFlow::<()>::Continue(())
        });
        if result.failure.is_some() {
            read_failed(Path::new("session.jsonl"), diagnostics);
        }
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

    fn reject_last_line(path: &Path, diagnostics: &Diagnostics) {
        let mut rejected_lines = 0;
        let result = JsonLines::open(path, DEFAULT_BUFFER_SIZE, false)
            .unwrap()
            .scan(Tail::Keep, |line| {
                if line.unterminated() {
                    rejected_lines += 1;
                    rejected(path, line, diagnostics);
                }
                ControlFlow::<()>::Continue(())
            });
        assert!(result.failure.is_none());
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
            reject_terminated(line, &diagnostics);
        }
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 0);
        assert_eq!(diagnostics.skipped(SkipKind::UndecodedUsage), 0);
    }

    #[test]
    fn a_rejected_wrapped_usage_record_is_invalid_data() {
        let diagnostics = Diagnostics::new("claude");
        reject_terminated(
            br#"{"data":{"message":{"message":{"usage":{}}}}}"#,
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
            reject_terminated(line, &diagnostics);
            assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        }
    }
}
