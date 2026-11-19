use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use crate::{
    TimestampMs,
    cli::{ReportClock, SharedArgs},
    log_level,
    output::sanitize_terminal_text,
};

const SAMPLES_PER_KIND: usize = 5;

/// Classifies files and records skipped while loading usage.
/// Use [`Self::UndecodedUsage`] until a record is confirmed to carry usage.
/// A confirmed usage record that fails to decode is [`Self::InvalidData`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkipKind {
    Unreadable,
    ReadInterrupted,
    UndecodedUsage,
    InvalidTimestamp,
    Invariant,
    InvalidData,
    UnreadableForkParent,
}

impl SkipKind {
    const ALL: [Self; 7] = [
        Self::Unreadable,
        Self::ReadInterrupted,
        Self::UndecodedUsage,
        Self::InvalidTimestamp,
        Self::Invariant,
        Self::InvalidData,
        Self::UnreadableForkParent,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Unreadable => "unreadable",
            Self::ReadInterrupted => "read interrupted",
            Self::UndecodedUsage => "undecoded usage",
            Self::InvalidTimestamp => "invalid timestamp",
            Self::Invariant => "unexpected shape",
            Self::InvalidData => "invalid data",
            Self::UnreadableForkParent => "unreadable fork parent",
        }
    }

    fn summary(self, tally: &Tally) -> String {
        if self == Self::UnreadableForkParent {
            let sessions = counted(tally.whole_units(), "forked session");
            return format!(
                "could not read the parent rollout for {sessions}. Replayed usage may be counted twice."
            );
        }
        format!("skipped {} ({})", tally.skipped_description(), self.label())
    }
}

#[derive(Clone, Debug)]
pub struct Skip {
    unit: PathBuf,
    line: Option<u32>,
    row_id: Option<Arc<str>>,
    table: Option<&'static str>,
    kind: SkipKind,
    detail: String,
}

impl Skip {
    pub fn unit(unit: &Path, kind: SkipKind, detail: impl fmt::Display) -> Self {
        Self {
            unit: unit.to_path_buf(),
            line: None,
            row_id: None,
            table: None,
            kind,
            detail: detail.to_string(),
        }
    }

    pub fn record(unit: &Path, line: u32, kind: SkipKind, detail: impl fmt::Display) -> Self {
        Self {
            line: Some(line),
            ..Self::unit(unit, kind, detail)
        }
    }

    pub fn line_number(index: usize) -> u32 {
        u32::try_from(index.saturating_add(1)).unwrap_or(u32::MAX)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum RecordIdentity {
    Line(u32),
    Row(Arc<str>),
}

impl Skip {
    fn identity(&self) -> Option<RecordIdentity> {
        self.row_id
            .as_ref()
            .map(|id| RecordIdentity::Row(Arc::clone(id)))
            .or_else(|| self.line.map(RecordIdentity::Line))
    }
}

#[derive(Default)]
struct Tally {
    units: BTreeMap<PathBuf, UnitSkips>,
    samples: Vec<Skip>,
}

#[derive(Default)]
struct UnitSkips {
    whole: bool,
    records: BTreeSet<(Option<&'static str>, RecordIdentity)>,
}

impl UnitSkips {
    fn record(&mut self, table: Option<&'static str>, line: Option<RecordIdentity>) {
        match line {
            Some(line) => {
                self.records.insert((table, line));
            }
            None => self.whole = true,
        }
    }
}

impl Tally {
    fn count(&mut self, unit: &Path, table: Option<&'static str>, line: Option<RecordIdentity>) {
        if let Some(skips) = self.units.get_mut(unit) {
            skips.record(table, line);
            return;
        }
        self.units
            .entry(unit.to_path_buf())
            .or_default()
            .record(table, line);
    }

    fn sample(&mut self, skip: Skip) {
        if let Some(sample) = self.samples.iter_mut().find(|sample| {
            sample.unit == skip.unit
                && sample.table == skip.table
                && sample.identity() == skip.identity()
        }) {
            if sample_order(&skip, sample).is_lt() {
                *sample = skip;
            }
            return;
        }
        if self.samples.len() < SAMPLES_PER_KIND
            || self
                .samples
                .last()
                .is_some_and(|sample| sample_order(&skip, sample).is_lt())
        {
            self.samples.push(skip);
            self.samples.sort_by(sample_order);
            self.samples.truncate(SAMPLES_PER_KIND);
        }
    }

    fn merge(&mut self, other: &Self) {
        for (unit, skips) in &other.units {
            if skips.whole {
                self.count(unit, None, None);
            }
            for (table, line) in &skips.records {
                self.count(unit, *table, Some(line.clone()));
            }
        }
        self.samples.extend(other.samples.iter().cloned());
        self.samples.sort_by(sample_order);
        self.samples.dedup_by(|right, left| {
            left.unit == right.unit
                && left.table == right.table
                && left.identity() == right.identity()
        });
        self.samples.truncate(SAMPLES_PER_KIND);
    }

    fn records(&self) -> u64 {
        self.units
            .values()
            .map(|skips| skips.records.len() as u64)
            .sum()
    }

    fn whole_units(&self) -> u64 {
        self.units.values().filter(|skips| skips.whole).count() as u64
    }

    fn skips(&self) -> u64 {
        self.whole_units() + self.records()
    }

    fn skipped_description(&self) -> String {
        let inputs = counted(self.units.len() as u64, "input");
        let whole_inputs = self.whole_units();
        let records = self.records();
        if records == 0 {
            inputs
        } else if whole_inputs == 0 {
            format!("{} in {inputs}", counted(records, "record"))
        } else {
            format!(
                "{} and {} in {inputs}",
                counted(whole_inputs, "input"),
                counted(records, "record")
            )
        }
    }
}

#[derive(Default)]
struct State {
    notes: BTreeMap<PathBuf, String>,
    tallies: [Tally; SkipKind::ALL.len()],
}

fn sample_order(left: &Skip, right: &Skip) -> std::cmp::Ordering {
    left.unit
        .cmp(&right.unit)
        .then_with(|| left.table.cmp(&right.table))
        .then_with(|| left.identity().cmp(&right.identity()))
        .then_with(|| left.detail.cmp(&right.detail))
}

pub struct Diagnostics {
    source: &'static str,
    assessment_instant: Option<TimestampMs>,
    table: Option<&'static str>,
    row_id: Option<Arc<str>>,
    state: Arc<Mutex<State>>,
}

impl Diagnostics {
    /// Creates diagnostics pinned to the report clock for time-sensitive classifications.
    pub fn for_report(source: &'static str, clock: ReportClock) -> Self {
        Self::at(source, clock.now().into())
    }

    /// Creates diagnostics with an explicit assessment instant.
    pub fn at(source: &'static str, assessment_instant: TimestampMs) -> Self {
        Self::build(source, Some(assessment_instant))
    }

    /// Creates diagnostics without an assessment instant for time-independent discovery.
    pub fn without_clock(source: &'static str) -> Self {
        Self::build(source, None)
    }

    fn build(source: &'static str, assessment_instant: Option<TimestampMs>) -> Self {
        Self {
            source,
            assessment_instant,
            table: None,
            row_id: None,
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    pub fn assessment_instant(&self) -> Option<TimestampMs> {
        self.assessment_instant
    }

    pub fn isolated(&self) -> Self {
        Self::build(self.source, self.assessment_instant)
    }

    pub fn source(&self) -> &'static str {
        self.source
    }

    pub fn for_table(&self, table: &'static str) -> Self {
        Self {
            source: self.source,
            assessment_instant: self.assessment_instant,
            table: Some(table),
            row_id: self.row_id.clone(),
            state: Arc::clone(&self.state),
        }
    }

    pub fn for_row(&self, id: Option<&str>) -> Self {
        Self {
            source: self.source,
            assessment_instant: self.assessment_instant,
            table: self.table,
            row_id: id.map(Arc::from),
            state: Arc::clone(&self.state),
        }
    }

    pub fn record(&self, mut skip: Skip) {
        if skip.line.is_some() {
            skip.table = skip.table.or(self.table);
            skip.row_id = skip.row_id.or_else(|| self.row_id.clone());
        }
        let mut state = self.state();
        let tally = &mut state.tallies[skip.kind as usize];
        tally.count(&skip.unit, skip.table, skip.identity());
        tally.sample(skip);
    }

    /// Records an observation shown only in debug output without counting a skip.
    pub fn debug_note(&self, unit: &Path, note: impl Into<String>) {
        self.state()
            .notes
            .entry(unit.to_path_buf())
            .or_insert_with(|| note.into());
    }

    /// Combines diagnostics from the same source, counting duplicate skips once.
    pub fn merge(&self, other: &Self) {
        debug_assert_eq!(self.source, other.source);
        if Arc::ptr_eq(&self.state, &other.state) {
            return;
        }
        let other = other.state();
        let mut state = self.state();
        for (unit, note) in &other.notes {
            state
                .notes
                .entry(unit.clone())
                .or_insert_with(|| note.clone());
        }
        for (tally, other) in state.tallies.iter_mut().zip(&other.tallies) {
            tally.merge(other);
        }
    }

    pub fn skipped(&self, kind: SkipKind) -> u64 {
        self.state().tallies[kind as usize].skips()
    }

    pub fn emit(&self, debug: bool) {
        for warning in self.render(log_level() != Some(0), debug) {
            eprintln!("{warning}");
        }
    }

    /// Returns aggregated warnings plus per-unit details in debug mode.
    pub fn warnings(&self, shared: &SharedArgs) -> Vec<String> {
        self.render(log_level() != Some(0), shared.debug)
    }

    fn render(&self, warn: bool, debug: bool) -> Vec<String> {
        let state = self.state();
        let mut lines = if debug {
            state.notes.values().cloned().collect()
        } else {
            Vec::new()
        };
        for (kind, tally) in SkipKind::ALL.into_iter().zip(&state.tallies) {
            if tally.units.is_empty() {
                continue;
            }
            if warn && (kind != SkipKind::UndecodedUsage || debug) {
                lines.push(format!("WARN  {}: {}", self.source, kind.summary(tally)));
            }
            if debug {
                lines.extend(tally.samples.iter().map(|skip| self.sample_line(skip)));
                let unlisted = tally.skips() - tally.samples.len() as u64;
                if unlisted > 0 {
                    lines.push(format!(
                        "{}: {}: {unlisted} more not listed",
                        self.source,
                        kind.label()
                    ));
                }
            }
        }
        lines
            .into_iter()
            .map(|line| sanitize_terminal_text(&line))
            .collect()
    }

    fn sample_line(&self, skip: &Skip) -> String {
        let table = skip
            .table
            .map(|table| format!(":{table}"))
            .unwrap_or_default();
        let line = match skip.identity() {
            Some(RecordIdentity::Line(line)) => format!(":{line}"),
            Some(RecordIdentity::Row(id)) => format!(":{id}"),
            None => String::new(),
        };
        format!(
            "{}: {}: {}{table}{line}: {}",
            self.source,
            skip.kind.label(),
            skip.unit.display(),
            skip.detail
        )
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn counted(count: u64, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

#[cfg(test)]
mod tests {
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
        let diagnostics = Diagnostics::at("opencode", crate::utc_now()).for_table("message");
        for (id, ordinal) in [("row-a", 1), ("row-a", 7), ("row-b", 1)] {
            diagnostics.for_row(Some(id)).record(Skip::record(
                Path::new("db.sqlite"),
                ordinal,
                SkipKind::InvalidTimestamp,
                "missing time",
            ));
        }
        assert_eq!(diagnostics.skipped(SkipKind::InvalidTimestamp), 2);
        let other = Diagnostics::at("opencode", crate::utc_now()).for_table("message");
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
    fn for_report_uses_the_captured_clock() {
        let now: jiff::Timestamp = "2026-01-02T00:00:00Z".parse().unwrap();
        let diagnostics = Diagnostics::for_report("test", crate::cli::ReportClock::Fixed(now));

        assert_eq!(
            diagnostics.assessment_instant(),
            Some(crate::TimestampMs::from_millis(now.as_millisecond()))
        );
    }

    #[test]
    fn healthy_loads_render_nothing() {
        let diagnostics = Diagnostics::at("codex", crate::utc_now());

        assert!(diagnostics.render(true, true).is_empty());
    }

    #[test]
    fn aggregates_record_skips_into_one_line_per_kind() {
        let diagnostics = Diagnostics::at("codex", crate::utc_now());
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
        let diagnostics = Diagnostics::at("opencode", crate::utc_now());
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

        let repeated = Diagnostics::at("opencode", crate::utc_now());
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
        let diagnostics = Diagnostics::at("amp", crate::utc_now());
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
        let diagnostics = Diagnostics::at("test", crate::utc_now());
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
        let diagnostics = Diagnostics::at("test", crate::utc_now());
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
        let narrow = Diagnostics::at("claude", crate::utc_now());
        narrow.record(invalid_timestamp("/logs/new.jsonl", 2));
        let wide = Diagnostics::at("claude", crate::utc_now());
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
        let left = Diagnostics::at("test", crate::utc_now());
        let right = Diagnostics::at("test", crate::utc_now());
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
        let diagnostics = Diagnostics::at("codex", crate::utc_now());
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
        let diagnostics = Diagnostics::at("test", crate::utc_now());
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
        let diagnostics = Diagnostics::at("codex", crate::utc_now());
        diagnostics.record(invalid_timestamp("/logs/a.jsonl", 3));

        assert!(diagnostics.render(false, false).is_empty());
    }

    #[test]
    fn undecoded_usage_is_counted_but_printed_only_under_debug() {
        let diagnostics = Diagnostics::at("claude", crate::utc_now());
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
        let diagnostics = Diagnostics::at("amp", crate::utc_now());
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
        .map(|(unit, line, detail)| {
            Skip::record(Path::new(unit), line, SkipKind::InvalidData, detail)
        });
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
            let diagnostics = Diagnostics::at("test", crate::utc_now());
            for index in order {
                diagnostics.record(skips[index].clone());
            }

            assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 7);
            assert_eq!(diagnostics.render(true, true), expected);
        }
    }

    #[test]
    fn debug_notes_come_first_and_only_under_debug() {
        let diagnostics = Diagnostics::at("antigravity", crate::utc_now());
        diagnostics.record(invalid_timestamp("/data/a.db", 1));
        diagnostics.debug_note(
            Path::new("/data/b.db"),
            "Skipping empty database '/data/b.db'",
        );
        let again = Diagnostics::at("antigravity", crate::utc_now());
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
}
