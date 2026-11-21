use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use crate::{cli::SharedArgs, log_level, output::sanitize_terminal_text};

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
    MissingForkParent,
}

impl SkipKind {
    const ALL: [Self; 8] = [
        Self::Unreadable,
        Self::ReadInterrupted,
        Self::UndecodedUsage,
        Self::InvalidTimestamp,
        Self::Invariant,
        Self::InvalidData,
        Self::UnreadableForkParent,
        Self::MissingForkParent,
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
            Self::MissingForkParent => "missing fork parent",
        }
    }

    fn summary(self, tally: &Tally) -> String {
        let parent = match self {
            Self::UnreadableForkParent => "read",
            Self::MissingForkParent => "find",
            Self::Unreadable
            | Self::ReadInterrupted
            | Self::UndecodedUsage
            | Self::InvalidTimestamp
            | Self::Invariant
            | Self::InvalidData => {
                return format!("skipped {} ({})", tally.skipped_description(), self.label());
            }
        };
        let sessions = counted(tally.whole_units(), "forked session");
        format!(
            "could not {parent} the parent rollout for {sessions}. Replayed usage may be counted twice."
        )
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
    table: Option<&'static str>,
    row_id: Option<Arc<str>>,
    state: Arc<Mutex<State>>,
}

impl Diagnostics {
    pub fn new(source: &'static str) -> Self {
        Self {
            source,
            table: None,
            row_id: None,
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    pub fn isolated(&self) -> Self {
        Self::new(self.source)
    }

    pub fn source(&self) -> &'static str {
        self.source
    }

    pub fn for_table(&self, table: &'static str) -> Self {
        Self {
            source: self.source,
            table: Some(table),
            row_id: self.row_id.clone(),
            state: Arc::clone(&self.state),
        }
    }

    pub fn for_row(&self, id: Option<&str>) -> Self {
        Self {
            source: self.source,
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
            ccusage_terminal::errln!("{warning}");
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
mod tests;
