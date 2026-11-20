use super::time::{format_block_end, format_block_start, format_block_started};
use super::*;
use ccusage_core::cli::SharedArgs;
use ccusage_test_support::{Environment, fs_fixture};

#[test]
fn fractional_session_hours_keep_exact_millisecond_boundaries() {
    let fixture = fs_fixture!({
        "projects/p/s.jsonl": [
            r#"{"timestamp":"2026-01-02T10:00:00Z","message":{"usage":{"input_tokens":10,"output_tokens":1}}}"#,
            r#"{"timestamp":"2026-01-02T15:00:00Z","message":{"usage":{"input_tokens":20,"output_tokens":1}}}"#,
        ].join("\n"),
    });
    Environment::with_var("CLAUDE_CONFIG_DIR", fixture.root()).run(|| {
        let entries = crate::load_entries(
            &SharedArgs {
                mode: ccusage_core::cli::CostMode::Display,
                ..SharedArgs::default()
            },
            None,
        )
        .unwrap();
        let now = crate::parse_ts_timestamp("2026-01-03T00:00:00Z").unwrap();
        let blocks = identify_session_blocks(entries, "4.35".parse().unwrap(), now);
        assert_eq!(blocks.len(), 3);
        assert_eq!(
            block_json(&blocks[0], None, now)["endTime"],
            "2026-01-02T14:21:00.000Z"
        );
        assert!(blocks[1].is_gap);
        assert_eq!(
            block_json(&blocks[1], None, now)["startTime"],
            "2026-01-02T14:21:00.000Z"
        );
    })
}

#[test]
fn saturated_block_buckets_do_not_overflow_the_burn_rate() {
    let fixture = fs_fixture!({
        "projects/p/s.jsonl": [
            r#"{"timestamp":"2026-01-02T10:00:00Z","message":{"usage":{"input_tokens":18446744073709551615,"output_tokens":0}}}"#,
            r#"{"timestamp":"2026-01-02T10:05:00Z","message":{"usage":{"input_tokens":0,"output_tokens":18446744073709551615}}}"#,
        ]
        .join("\n"),
    });
    Environment::with_var("CLAUDE_CONFIG_DIR", fixture.root()).run(|| {
        let entries = crate::load_entries(
            &SharedArgs {
                mode: ccusage_core::cli::CostMode::Display,
                ..SharedArgs::default()
            },
            None,
        )
        .unwrap();
        let now = crate::parse_ts_timestamp("2026-01-02T10:06:00Z").unwrap();
        let blocks = identify_session_blocks(entries, "4.35".parse().unwrap(), now);

        let json = block_json(&blocks[0], None, now);

        assert_eq!(
            json["burnRate"]["tokensPerMinuteForIndicator"].as_f64(),
            Some(u64::MAX as f64 / 5.0)
        );
    })
}

#[test]
fn block_times_follow_the_report_timezone_across_midnight() {
    let timestamp = crate::parse_ts_timestamp("2026-01-02T23:30:45Z").unwrap();
    let zone = jiff::tz::TimeZone::get("Asia/Tokyo").unwrap();
    assert_eq!(
        format_block_start(timestamp, false, &zone).unwrap(),
        "1/3/2026, 8:30:45 AM"
    );
    assert_eq!(
        format_block_start(timestamp, true, &zone).unwrap(),
        "01/03, 08:30 AM"
    );
    assert_eq!(
        format_block_end(timestamp, false, &zone).unwrap(),
        "1/3/2026, 8:30:45 AM"
    );
    assert_eq!(
        format_block_end(timestamp, true, &zone).unwrap(),
        "08:30 AM"
    );
    assert_eq!(
        format_block_started(timestamp, &zone).unwrap(),
        "2026-01-03 08:30:45"
    );
}

#[test]
fn block_times_follow_the_report_timezone_dst_transition() {
    let zone = jiff::tz::TimeZone::get("Europe/London").unwrap();
    let before = crate::parse_ts_timestamp("2026-03-29T00:30:00Z").unwrap();
    let after = crate::parse_ts_timestamp("2026-03-29T01:30:00Z").unwrap();
    assert_eq!(
        format_block_started(before, &zone).unwrap(),
        "2026-03-29 00:30:00"
    );
    assert_eq!(
        format_block_started(after, &zone).unwrap(),
        "2026-03-29 02:30:00"
    );
}

#[test]
fn only_a_pause_longer_than_the_session_creates_a_gap() {
    for (next, expected_blocks) in [
        ("2026-01-02T15:00:00.000Z", 1),
        ("2026-01-02T15:00:00.001Z", 3),
    ] {
        let fixture = fs_fixture!({
            "projects/p/s.jsonl": format!(r#"{{"timestamp":"2026-01-02T10:00:00Z","message":{{"usage":{{"input_tokens":10,"output_tokens":1}}}}}}
            {{"timestamp":"{next}","message":{{"usage":{{"input_tokens":20,"output_tokens":1}}}}}}"#),
        });
        Environment::with_var("CLAUDE_CONFIG_DIR", fixture.root()).run(|| {
            let entries = crate::load_entries(
                &SharedArgs {
                    mode: ccusage_core::cli::CostMode::Display,
                    ..SharedArgs::default()
                },
                None,
            )
            .unwrap();
            let blocks = identify_session_blocks(
                entries,
                SessionLength::DEFAULT,
                crate::parse_ts_timestamp("2026-01-02T16:00:00Z").unwrap(),
            );
            assert_eq!(blocks.len(), expected_blocks, "{next}");
            assert_eq!(
                blocks.iter().filter(|block| block.is_gap).count(),
                expected_blocks / 3
            );
            assert_eq!(
                blocks
                    .iter()
                    .map(|block| block.token_counts.input_tokens)
                    .sum::<u64>(),
                30
            );
        })
    }
}
