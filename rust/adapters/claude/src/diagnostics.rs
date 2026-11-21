use std::path::Path;

use ccusage_adapter_common::jsonl::Line;
use ccusage_core::{Diagnostics, Skip, SkipKind};

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
    use std::ops::ControlFlow;

    use ccusage_adapter_common::jsonl::{self, ReadPolicy};
    use ccusage_core::{cli::CostMode, cost::PriceTable};
    use ccusage_test_support::{instant, set_modified};

    #[test]
    fn a_file_removed_after_discovery_is_not_a_skip() {
        let fixture = ccusage_test_support::Fixture::new();
        let diagnostics = Diagnostics::new("claude");

        let pricing = ccusage_core::PricingMap::default();
        let entries = crate::daily::read_usage_file(
            &fixture.path("missing.jsonl"),
            None,
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
        let fixture = ccusage_test_support::Fixture::new();
        let path = fixture.path("session.jsonl");
        std::fs::write(&path, [bytes, b"\n"].concat()).unwrap();
        jsonl::read(&path, Some(diagnostics), ReadPolicy::PLAIN, |line, _, _| {
            rejected(&path, line, diagnostics);
            ControlFlow::<()>::Continue(())
        })
        .unwrap();
    }

    const CUT_LAST_LINE: &str = r#"{"message":{"usage":{"input_tokens":10}}}
{"message":{"usage":{"input_tok"#;

    fn reject_last_line(path: &Path, diagnostics: &Diagnostics, after_first_line: impl Fn()) {
        let mut rejected_lines = 0;
        jsonl::read(path, Some(diagnostics), ReadPolicy::PLAIN, |line, _, _| {
            if line.unterminated() {
                rejected_lines += 1;
                rejected(path, line, diagnostics);
            } else {
                after_first_line();
            }
            ControlFlow::<()>::Continue(())
        })
        .unwrap();
        assert_eq!(rejected_lines, 1);
    }

    #[test]
    fn a_usage_line_cut_short_while_the_file_grows_is_not_a_skip() {
        let fixture = ccusage_test_support::Fixture::new();
        let path = fixture.write_file("session.jsonl", CUT_LAST_LINE);
        let diagnostics = Diagnostics::new("claude");

        reject_last_line(&path, &diagnostics, || {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(b"ens")
                .unwrap();
        });

        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 0);
    }

    #[test]
    fn a_usage_line_left_cut_short_is_skipped_as_truncated_however_recent() {
        let now = instant("2026-01-02T00:00:00Z");
        for modified in [now, instant("2026-01-01T23:58:00Z")] {
            let fixture = ccusage_test_support::Fixture::new();
            let path = fixture.write_file("session.jsonl", CUT_LAST_LINE);
            set_modified(&path, modified);
            let diagnostics = Diagnostics::new("claude");

            reject_last_line(&path, &diagnostics, || {});

            assert_eq!(
                diagnostics.skipped(SkipKind::InvalidData),
                1,
                "{modified:?}"
            );
        }
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
