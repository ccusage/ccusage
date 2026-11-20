use std::path::Path;

use super::{Diagnostics, SAMPLES_PER_KIND, Skip, SkipKind};

fn invalid_timestamp(unit: &str, line: u32) -> Skip {
    Skip::record(
        Path::new(unit),
        line,
        SkipKind::InvalidTimestamp,
        "timestamp 'soon'",
    )
}

#[test]
fn stable_database_row_ids_survive_changed_scan_ordinals() {
    let diagnostics = Diagnostics::new("opencode").for_table("message");
    for (id, ordinal) in [("row-a", 1), ("row-a", 7), ("row-b", 1)] {
        diagnostics.for_row(Some(id)).record(Skip::record(
            Path::new("db.sqlite"),
            ordinal,
            SkipKind::InvalidTimestamp,
            "missing time",
        ));
    }
    assert_eq!(diagnostics.skipped(SkipKind::InvalidTimestamp), 2);
    let other = Diagnostics::new("opencode").for_table("message");
    other.for_row(Some("row-a")).record(Skip::record(
        Path::new("db.sqlite"),
        3,
        SkipKind::InvalidTimestamp,
        "missing time",
    ));
    diagnostics.merge(&other);
    assert_eq!(diagnostics.skipped(SkipKind::InvalidTimestamp), 2);
    assert!(
        diagnostics
            .render(true, true)
            .iter()
            .any(|line| line.contains("message:row-a"))
    );
}

#[test]
fn line_numbers_saturate_instead_of_wrapping() {
    assert_eq!(Skip::line_number(0), 1);
    assert_eq!(Skip::line_number(usize::MAX), u32::MAX);
}

#[test]
fn tallies_are_indexed_by_declaration_order() {
    for (index, kind) in SkipKind::ALL.into_iter().enumerate() {
        assert_eq!(kind as usize, index);
    }
}

#[test]
fn healthy_loads_render_nothing() {
    let diagnostics = Diagnostics::new("codex");

    assert!(diagnostics.render(true, true).is_empty());
}

#[test]
fn aggregates_record_skips_into_one_line_per_kind() {
    let diagnostics = Diagnostics::new("codex");
    diagnostics.record(invalid_timestamp("/logs/a.jsonl", 3));
    diagnostics.record(invalid_timestamp("/logs/a.jsonl", 9));
    diagnostics.record(invalid_timestamp("/logs/b.jsonl", 1));

    assert_eq!(
        diagnostics.render(true, false),
        ["WARN  codex: skipped 3 records in 2 inputs (invalid timestamp)"]
    );
    assert_eq!(diagnostics.skipped(SkipKind::InvalidTimestamp), 3);
}

#[test]
fn table_rows_share_the_database_input_and_keep_distinct_identities() {
    let diagnostics = Diagnostics::new("opencode");
    let message = diagnostics.for_table("message");
    let session_message = diagnostics.for_table("session_message");
    message.record(invalid_timestamp("/logs/opencode.db", 1));
    session_message.record(invalid_timestamp("/logs/opencode.db", 1));
    message.record(invalid_timestamp("/logs/opencode.db", 1));
    diagnostics.merge(&message);
    assert_eq!(diagnostics.skipped(SkipKind::InvalidTimestamp), 2);
    assert_eq!(
        diagnostics.render(true, true),
        [
            "WARN  opencode: skipped 2 records in 1 input (invalid timestamp)",
            "opencode: invalid timestamp: /logs/opencode.db:message:1: timestamp 'soon'",
            "opencode: invalid timestamp: /logs/opencode.db:session_message:1: timestamp 'soon'",
        ]
    );

    let repeated = Diagnostics::new("opencode");
    repeated
        .for_table("message")
        .record(invalid_timestamp("/logs/opencode.db", 1));
    repeated.record(Skip::unit(
        Path::new("/logs/opencode.db"),
        SkipKind::InvalidTimestamp,
        "database rejected",
    ));
    diagnostics.merge(&repeated);
    assert_eq!(diagnostics.skipped(SkipKind::InvalidTimestamp), 3);
    assert_eq!(
        diagnostics.render(true, false),
        ["WARN  opencode: skipped 1 input and 2 records in 1 input (invalid timestamp)"]
    );
}

#[test]
fn whole_unit_skips_count_files_and_keep_singulars() {
    let diagnostics = Diagnostics::new("amp");
    diagnostics.record(Skip::unit(
        Path::new("/threads/T-1.json"),
        SkipKind::Unreadable,
        "permission denied",
    ));
    diagnostics.record(invalid_timestamp("/threads/T-2.json", 1));

    assert_eq!(
        diagnostics.render(true, false),
        [
            "WARN  amp: skipped 1 input (unreadable)",
            "WARN  amp: skipped 1 record in 1 input (invalid timestamp)",
        ]
    );
}

#[test]
fn mixed_skips_count_whole_units_separately_from_records() {
    let diagnostics = Diagnostics::new("test");
    diagnostics.record(Skip::unit(
        Path::new("/logs/broken.json"),
        SkipKind::InvalidData,
        "malformed JSON",
    ));
    diagnostics.record(Skip::record(
        Path::new("/logs/partial.jsonl"),
        3,
        SkipKind::InvalidData,
        "malformed JSON",
    ));

    assert_eq!(
        diagnostics.render(true, false),
        ["WARN  test: skipped 1 input and 1 record in 2 inputs (invalid data)"]
    );
}

#[test]
fn a_skip_recorded_twice_counts_once() {
    let diagnostics = Diagnostics::new("test");
    diagnostics.record(Skip::unit(
        Path::new("/logs/broken.json"),
        SkipKind::InvalidData,
        "malformed JSON",
    ));
    diagnostics.record(Skip::unit(
        Path::new("/logs/broken.json"),
        SkipKind::InvalidData,
        "could not parse JSON",
    ));
    diagnostics.record(invalid_timestamp("/logs/partial.jsonl", 3));
    diagnostics.record(invalid_timestamp("/logs/partial.jsonl", 3));

    assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
    assert_eq!(diagnostics.skipped(SkipKind::InvalidTimestamp), 1);
    assert_eq!(
        diagnostics.render(true, true),
        [
            "WARN  test: skipped 1 record in 1 input (invalid timestamp)",
            "test: invalid timestamp: /logs/partial.jsonl:3: timestamp 'soon'",
            "WARN  test: skipped 1 input (invalid data)",
            "test: invalid data: /logs/broken.json: could not parse JSON",
        ]
    );
}

#[test]
fn merging_two_loads_of_one_source_counts_each_skip_once() {
    let narrow = Diagnostics::new("claude");
    narrow.record(invalid_timestamp("/logs/new.jsonl", 2));
    let wide = Diagnostics::new("claude");
    wide.record(invalid_timestamp("/logs/new.jsonl", 2));
    wide.record(invalid_timestamp("/logs/old.jsonl", 1));
    wide.record(invalid_timestamp("/logs/old.jsonl", 5));

    narrow.merge(&wide);

    assert_eq!(narrow.skipped(SkipKind::InvalidTimestamp), 3);
    assert_eq!(
        narrow.render(true, true),
        [
            "WARN  claude: skipped 3 records in 2 inputs (invalid timestamp)",
            "claude: invalid timestamp: /logs/new.jsonl:2: timestamp 'soon'",
            "claude: invalid timestamp: /logs/old.jsonl:1: timestamp 'soon'",
            "claude: invalid timestamp: /logs/old.jsonl:5: timestamp 'soon'",
        ]
    );
}

#[test]
fn merging_keeps_the_first_samples_by_position() {
    let left = Diagnostics::new("test");
    let right = Diagnostics::new("test");
    for line in 1..=SAMPLES_PER_KIND as u32 {
        left.record(invalid_timestamp("/logs/b.jsonl", line));
        right.record(invalid_timestamp("/logs/a.jsonl", line));
    }

    left.merge(&right);
    let lines = left.render(true, true);

    assert_eq!(
        lines[0],
        format!(
            "WARN  test: skipped {} records in 2 inputs (invalid timestamp)",
            2 * SAMPLES_PER_KIND
        )
    );
    assert!(
        lines[1..=SAMPLES_PER_KIND]
            .iter()
            .all(|line| line.contains("/logs/a.jsonl:"))
    );
    assert_eq!(
        lines[lines.len() - 1],
        format!("test: invalid timestamp: {SAMPLES_PER_KIND} more not listed")
    );
}

#[test]
fn invalid_data_is_reported_by_default() {
    let diagnostics = Diagnostics::new("codex");
    diagnostics.record(Skip::unit(
        Path::new("/logs/session.json"),
        SkipKind::InvalidData,
        "JSON syntax error at line 1, column 12",
    ));

    assert_eq!(
        diagnostics.render(true, false),
        ["WARN  codex: skipped 1 input (invalid data)"]
    );
    assert_eq!(
        diagnostics.render(true, true),
        [
            "WARN  codex: skipped 1 input (invalid data)",
            "codex: invalid data: /logs/session.json: JSON syntax error at line 1, column 12",
        ]
    );
}

#[test]
fn rendered_diagnostics_escape_terminal_controls_in_dynamic_text() {
    let diagnostics = Diagnostics::new("test");
    diagnostics.debug_note(Path::new("/data/a.db"), "source path \u{1b}[2J");
    diagnostics.record(Skip::unit(
        Path::new("/logs/bad\u{1b}[2J.json"),
        SkipKind::InvalidData,
        "invalid \u{1b}[31m JSON",
    ));

    assert_eq!(
        diagnostics.render(true, true),
        [
            r"source path \u{1b}[2J",
            "WARN  test: skipped 1 input (invalid data)",
            r"test: invalid data: /logs/bad\u{1b}[2J.json: invalid \u{1b}[31m JSON",
        ]
    );
}

#[test]
fn log_level_zero_silences_the_aggregated_lines() {
    let diagnostics = Diagnostics::new("codex");
    diagnostics.record(invalid_timestamp("/logs/a.jsonl", 3));

    assert!(diagnostics.render(false, false).is_empty());
}

#[test]
fn undecoded_usage_is_counted_but_printed_only_under_debug() {
    let diagnostics = Diagnostics::new("claude");
    diagnostics.record(Skip::record(
        Path::new("/projects/p/s.jsonl"),
        7,
        SkipKind::UndecodedUsage,
        "missing field `output_tokens`",
    ));

    assert_eq!(diagnostics.skipped(SkipKind::UndecodedUsage), 1);
    assert!(diagnostics.render(true, false).is_empty());
    assert_eq!(
        diagnostics.render(true, true),
        [
            "WARN  claude: skipped 1 record in 1 input (undecoded usage)",
            "claude: undecoded usage: /projects/p/s.jsonl:7: missing field `output_tokens`",
        ]
    );
}

#[test]
fn debug_lists_a_few_units_and_counts_the_rest() {
    let diagnostics = Diagnostics::new("amp");
    let total = SAMPLES_PER_KIND + 2;
    for index in 0..total {
        diagnostics.record(Skip::unit(
            Path::new(&format!("/threads/T-{index}.json")),
            SkipKind::Unreadable,
            "permission denied",
        ));
    }

    let lines = diagnostics.render(true, true);

    assert_eq!(
        lines[0],
        format!("WARN  amp: skipped {total} inputs (unreadable)")
    );
    assert_eq!(
        lines[1],
        "amp: unreadable: /threads/T-0.json: permission denied"
    );
    assert_eq!(lines.len(), 1 + SAMPLES_PER_KIND + 1);
    assert_eq!(lines[lines.len() - 1], "amp: unreadable: 2 more not listed");
}

#[test]
fn debug_samples_are_stable_and_bounded_across_recording_orders() {
    let skips = [
        ("/logs/z.jsonl", 1, "z-one"),
        ("/logs/a.jsonl", 4, "a-four"),
        ("/logs/a.jsonl", 2, "a-beta"),
        ("/logs/b.jsonl", 2, "b-two"),
        ("/logs/a.jsonl", 2, "a-alpha"),
        ("/logs/c.jsonl", 1, "c-one"),
        ("/logs/a.jsonl", 1, "a-one"),
        ("/logs/d.jsonl", 1, "d-one"),
    ]
    .map(|(unit, line, detail)| Skip::record(Path::new(unit), line, SkipKind::InvalidData, detail));
    let expected = [
        "WARN  test: skipped 7 records in 5 inputs (invalid data)",
        "test: invalid data: /logs/a.jsonl:1: a-one",
        "test: invalid data: /logs/a.jsonl:2: a-alpha",
        "test: invalid data: /logs/a.jsonl:4: a-four",
        "test: invalid data: /logs/b.jsonl:2: b-two",
        "test: invalid data: /logs/c.jsonl:1: c-one",
        "test: invalid data: 2 more not listed",
    ];
    let orders = [
        [0, 1, 2, 3, 4, 5, 6, 7],
        [7, 6, 5, 4, 3, 2, 1, 0],
        [3, 0, 6, 2, 7, 4, 1, 5],
    ];

    for order in orders {
        let diagnostics = Diagnostics::new("test");
        for index in order {
            diagnostics.record(skips[index].clone());
        }

        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 7);
        assert_eq!(diagnostics.render(true, true), expected);
    }
}

#[test]
fn debug_notes_come_first_and_only_under_debug() {
    let diagnostics = Diagnostics::new("antigravity");
    diagnostics.record(invalid_timestamp("/data/a.db", 1));
    diagnostics.debug_note(
        Path::new("/data/b.db"),
        "Skipping empty database '/data/b.db'",
    );
    let again = Diagnostics::new("antigravity");
    again.debug_note(
        Path::new("/data/b.db"),
        "Skipping empty database '/data/b.db'",
    );
    diagnostics.merge(&again);

    assert_eq!(
        diagnostics.render(true, false),
        ["WARN  antigravity: skipped 1 record in 1 input (invalid timestamp)"]
    );
    assert_eq!(
        diagnostics.render(false, true),
        [
            "Skipping empty database '/data/b.db'",
            "antigravity: invalid timestamp: /data/a.db:1: timestamp 'soon'",
        ]
    );
}
