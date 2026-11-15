use std::{
    collections::BTreeSet,
    fmt,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

use crate::{cli::SharedArgs, log_level};

const SAMPLES_PER_KIND: usize = 5;

/// Why a file, database or record was left out of a report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkipKind {
    /// The unit could not be opened or read at all.
    Unreadable,
    /// Reading stopped part-way; what was read before the failure is kept.
    ReadInterrupted,
    /// A selected record did not decode and was not confirmed to carry usage.
    /// Such records may be rejected by design, so only `--debug` prints them.
    UndecodedUsage,
    /// A record carried a timestamp the report cannot place on a day.
    InvalidTimestamp,
    /// The input broke an assumption the loader relies on.
    Invariant,
    /// Input was malformed, or a confirmed usage record did not decode.
    InvalidData,
}

impl SkipKind {
    const ALL: [Self; 6] = [
        Self::Unreadable,
        Self::ReadInterrupted,
        Self::UndecodedUsage,
        Self::InvalidTimestamp,
        Self::Invariant,
        Self::InvalidData,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Unreadable => "unreadable",
            Self::ReadInterrupted => "read interrupted",
            Self::UndecodedUsage => "undecoded usage",
            Self::InvalidTimestamp => "invalid timestamp",
            Self::Invariant => "unexpected shape",
            Self::InvalidData => "invalid data",
        }
    }
}

/// One thing a loader left out, and where.
#[derive(Clone, Debug)]
pub struct Skip {
    /// The file or database the skip happened in.
    pub unit: PathBuf,
    /// The 1-based line or row inside the unit; `None` when the whole unit, or
    /// the rest of it, was skipped.
    pub line: Option<u32>,
    pub kind: SkipKind,
    pub detail: String,
}

impl Skip {
    /// A skip of a whole unit.
    pub fn unit(unit: &Path, kind: SkipKind, detail: impl fmt::Display) -> Self {
        Self {
            unit: unit.to_path_buf(),
            line: None,
            kind,
            detail: detail.to_string(),
        }
    }

    /// A skip of one record inside a unit that is otherwise read.
    pub fn record(unit: &Path, line: u32, kind: SkipKind, detail: impl fmt::Display) -> Self {
        Self {
            line: Some(line),
            ..Self::unit(unit, kind, detail)
        }
    }
}

#[derive(Default)]
struct Tally {
    skips: u64,
    records: u64,
    units: BTreeSet<PathBuf>,
    samples: Vec<Skip>,
}

#[derive(Default)]
struct State {
    lines: Vec<String>,
    tallies: [Tally; SkipKind::ALL.len()],
}

/// What one source left out while loading one report.
///
/// Loaders record every skip here instead of dropping it silently or logging
/// it only under `--debug`, so input that could not be counted shows up as a
/// warning rather than as lower usage. Recording takes a lock, which is fine
/// because a skip is the cold path; a loader that reads healthy input never
/// touches it.
pub struct Diagnostics {
    source: &'static str,
    state: Mutex<State>,
}

impl Diagnostics {
    /// `source` is the agent id the warnings name, such as `codex`.
    pub fn new(source: &'static str) -> Self {
        Self {
            source,
            state: Mutex::new(State::default()),
        }
    }

    pub fn record(&self, skip: Skip) {
        let mut state = self.state();
        let tally = &mut state.tallies[skip.kind as usize];
        tally.skips += 1;
        if skip.line.is_some() {
            tally.records += 1;
        }
        if !tally.units.contains(&skip.unit) {
            tally.units.insert(skip.unit.clone());
        }
        if tally.samples.len() < SAMPLES_PER_KIND {
            tally.samples.push(skip);
        }
    }

    /// Adds a warning line a source already words and gates itself, printed
    /// as-is before the aggregated ones.
    pub fn push_line(&self, line: impl Into<String>) {
        self.state().lines.push(line.into());
    }

    /// How many skips of `kind` were recorded.
    pub fn skipped(&self, kind: SkipKind) -> u64 {
        self.state().tallies[kind as usize].skips
    }

    /// The lines to print on stderr once the load's progress output stopped:
    /// one aggregated `WARN` per kind, silenced by `LOG_LEVEL=0` like every
    /// other warning, plus the skipped units themselves under `--debug`.
    pub fn warnings(&self, shared: &SharedArgs) -> Vec<String> {
        self.render(log_level() != Some(0), shared.debug)
    }

    fn render(&self, warn: bool, debug: bool) -> Vec<String> {
        let state = self.state();
        let mut lines = state.lines.clone();
        for (kind, tally) in SkipKind::ALL.into_iter().zip(&state.tallies) {
            if tally.skips == 0 {
                continue;
            }
            if warn && (kind != SkipKind::UndecodedUsage || debug) {
                lines.push(self.summary_line(kind, tally));
            }
            if debug {
                lines.extend(tally.samples.iter().map(|skip| self.sample_line(skip)));
                let unlisted = tally.skips - tally.samples.len() as u64;
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
    }

    fn summary_line(&self, kind: SkipKind, tally: &Tally) -> String {
        let inputs = counted(tally.units.len() as u64, "input");
        let whole_units = tally.skips - tally.records;
        let skipped = if tally.records == 0 {
            inputs
        } else if whole_units == 0 {
            format!("{} in {inputs}", counted(tally.records, "record"))
        } else {
            format!(
                "{} and {} in {inputs}",
                counted(whole_units, "input"),
                counted(tally.records, "record")
            )
        };
        format!(
            "WARN  {}: skipped {skipped} ({})",
            self.source,
            kind.label()
        )
    }

    fn sample_line(&self, skip: &Skip) -> String {
        let line = skip.line.map(|line| format!(":{line}")).unwrap_or_default();
        format!(
            "{}: {}: {}{line}: {}",
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
    fn source_worded_lines_pass_through_before_the_aggregated_ones() {
        let diagnostics = Diagnostics::new("antigravity");
        diagnostics.record(invalid_timestamp("/data/a.db", 1));
        diagnostics.push_line("WARN  Skipping unreadable Antigravity database '/data/b.db': boom");

        assert_eq!(
            diagnostics.render(true, false),
            [
                "WARN  Skipping unreadable Antigravity database '/data/b.db': boom",
                "WARN  antigravity: skipped 1 record in 1 input (invalid timestamp)",
            ]
        );
        assert_eq!(
            diagnostics.render(false, false),
            ["WARN  Skipping unreadable Antigravity database '/data/b.db': boom"],
            "the source gated its own line already"
        );
    }
}
