use std::{
    cell::RefCell,
    fmt,
    fs::File,
    io::{self, BufReader, Read, Seek, SeekFrom},
    marker::PhantomData,
    ops::ControlFlow,
    path::{Path, PathBuf},
};

use serde::{
    Deserialize, Deserializer,
    de::{self, DeserializeOwned, IgnoredAny, MapAccess, SeqAccess, Visitor},
};

use ccusage_core::{
    Diagnostics, Skip, SkipKind,
    fast::{LinePrefilter, LineReader, newline_offsets},
};

use crate::{UnitError, units::being_written};

const DEFAULT_BUFFER_SIZE: usize = 8 * 1024;

thread_local! {
    static LINE_BUFFER: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

enum Input {
    Plain(File),
    Zstd(zstd::stream::read::Decoder<'static, BufReader<File>>),
}

impl Read for Input {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(file) => file.read(buffer),
            Self::Zstd(decoder) => decoder.read(buffer),
        }
    }
}

#[derive(Clone, Copy)]
pub struct ReadPolicy {
    pub capacity: usize,
    pub compressed: bool,
    pub retry_missing: Option<fn(&Path) -> PathBuf>,
    pub keep_interrupted_tail: fn(io::ErrorKind) -> bool,
    pub open_failure: Option<&'static str>,
    pub interrupted: fn(ReadFailure) -> Interruption,
    pub stage_diagnostics: bool,
}

impl ReadPolicy {
    pub const PLAIN: Self = Self {
        capacity: DEFAULT_BUFFER_SIZE,
        compressed: false,
        retry_missing: None,
        keep_interrupted_tail: |_| false,
        open_failure: None,
        interrupted: |failure| Interruption::Reject(failure.error.into()),
        stage_diagnostics: false,
    };
}

pub struct ReadFailure {
    pub error: io::Error,
    pub complete_lines: usize,
    pub tail_bytes: usize,
}

pub enum Interruption {
    Reject(UnitError),
    KeepUnit(UnitError),
    KeepRecords(SkipKind, &'static str),
    RejectRecords(SkipKind, &'static str, UnitError),
}

fn open_input(path: &Path, capacity: usize, compressed: bool) -> io::Result<Input> {
    let file = File::open(path)?;
    if compressed {
        zstd::stream::read::Decoder::with_buffer(BufReader::with_capacity(capacity, file))
            .map(Input::Zstd)
    } else {
        Ok(Input::Plain(file))
    }
}

#[inline(never)]
fn open<'a>(
    path: &'a Path,
    policy: ReadPolicy,
    diagnostics: Option<&Diagnostics>,
) -> Result<(Input, std::borrow::Cow<'a, Path>), UnitError> {
    use std::borrow::Cow;
    let (failure, error) = match (
        open_input(path, policy.capacity, policy.compressed),
        policy.retry_missing,
    ) {
        (Ok(input), _) => return Ok((input, Cow::Borrowed(path))),
        (Err(error), Some(retry)) if error.kind() == io::ErrorKind::NotFound => {
            let alternate = retry(path);
            match open_input(&alternate, policy.capacity, !policy.compressed) {
                Ok(input) => return Ok((input, Cow::Owned(alternate))),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    (Cow::Borrowed(path), error)
                }
                Err(error) => (Cow::Owned(alternate), error),
            }
        }
        (Err(error), _) => (Cow::Borrowed(path), error),
    };
    if let (Some(detail), Some(diagnostics)) = (policy.open_failure, diagnostics) {
        UnitError::new(SkipKind::Unreadable, detail).record(&failure, diagnostics);
    }
    Err(error.into())
}

#[inline(never)]
fn finish(
    path: &Path,
    diagnostics: Option<&Diagnostics>,
    staged: Option<&Diagnostics>,
    policy: ReadPolicy,
    failure: Option<ReadFailure>,
) -> Result<(), UnitError> {
    if let Some(failure) = failure {
        match (policy.interrupted)(failure) {
            Interruption::Reject(error) => return Err(error),
            Interruption::KeepUnit(error) => {
                if let Some(diagnostics) = diagnostics {
                    error.record(path, diagnostics);
                }
            }
            Interruption::RejectRecords(kind, detail, error) => {
                if let Some(diagnostics) = diagnostics {
                    diagnostics.record(Skip::unit(path, kind, detail));
                }
                return Err(error);
            }
            Interruption::KeepRecords(kind, detail) => {
                if let Some(diagnostics) = diagnostics {
                    diagnostics.record(Skip::unit(path, kind, detail));
                }
            }
        }
    }
    if let (Some(diagnostics), Some(staged)) = (diagnostics, staged) {
        diagnostics.merge(staged);
    }
    Ok(())
}

/// Reads one JSONL unit with the source's open, interruption and diagnostic policy.
pub fn read<B>(
    path: &Path,
    diagnostics: Option<&Diagnostics>,
    policy: ReadPolicy,
    mut visit: impl FnMut(Line<'_>, &Path, Option<&Diagnostics>) -> ControlFlow<B>,
) -> Result<Option<B>, UnitError> {
    let mut found = None;
    read_with(
        path,
        diagnostics,
        policy,
        &mut |line, opened_path, local| {
            if let ControlFlow::Break(value) = visit(line, opened_path, local) {
                found = Some(value);
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        },
    )?;
    Ok(found)
}

type LineVisitor<'a> = dyn FnMut(Line<'_>, &Path, Option<&Diagnostics>) -> ControlFlow<()> + 'a;

#[inline(never)]
fn read_with(
    path: &Path,
    diagnostics: Option<&Diagnostics>,
    policy: ReadPolicy,
    visit: &mut LineVisitor<'_>,
) -> Result<(), UnitError> {
    let (input, opened_path) = open(path, policy, diagnostics)?;
    let staged = if policy.stage_diagnostics {
        diagnostics.map(Diagnostics::isolated)
    } else {
        None
    };
    let local = staged.as_ref().or(diagnostics);
    let capacity = policy.capacity.max(1);
    let mut reader = line_reader(capacity, input);
    let mut index = 0;
    let failure = loop {
        match reader.next_line() {
            Ok(Some(raw)) => {
                if visit(Line { index, raw }, &opened_path, local).is_break() {
                    break None;
                }
            }
            Ok(None) => break None,
            Err(error) => {
                let tail = reader.rest();
                if !tail.is_empty() && (policy.keep_interrupted_tail)(error.kind()) {
                    let _ = visit(Line { index, raw: tail }, &opened_path, local);
                }
                break Some(ReadFailure {
                    error,
                    complete_lines: index,
                    tail_bytes: tail.len(),
                });
            }
        }
        index += 1;
    };
    recycle_line_buffer(capacity, reader);
    finish(path, diagnostics, staged.as_ref(), policy, failure)
}

#[inline(never)]
fn line_reader(capacity: usize, input: Input) -> LineReader<Input> {
    let mut buffer = LINE_BUFFER.take();
    if buffer.capacity() > capacity {
        buffer = Vec::new();
    }
    buffer.resize(capacity, 0);
    LineReader::with_buffer(buffer, input)
}

#[inline(never)]
fn recycle_line_buffer(capacity: usize, reader: LineReader<Input>) {
    let buffer = reader.into_buffer();
    if buffer.capacity() <= capacity {
        LINE_BUFFER.set(buffer);
    }
}

/// Visits physical lines from the end of a plain file until the visitor accepts one.
pub fn read_last<B>(
    path: &Path,
    mut visit: impl FnMut(ReverseLine<'_>) -> ControlFlow<B>,
) -> io::Result<Option<B>> {
    let mut file = File::open(path)?;
    let mut offset = file.metadata()?.len();
    let mut line_end = offset;
    let mut chunk = [0; DEFAULT_BUFFER_SIZE];
    let mut spanning = Vec::new();
    let mut number = None;

    while offset > 0 {
        let chunk_end = offset;
        offset = offset.saturating_sub(chunk.len() as u64);
        let chunk = &mut chunk[..(chunk_end - offset) as usize];
        read_range(&mut file, offset, chunk)?;
        let starts = newline_offsets(chunk)
            .rev()
            .map(|newline| newline + 1)
            .chain((offset == 0).then_some(0));
        for start in starts {
            let line_start = offset + start as u64;
            if line_start == line_end {
                continue;
            }
            let raw = if line_end <= chunk_end {
                &chunk[start..(line_end - offset) as usize]
            } else {
                let line_len = usize::try_from(line_end - line_start)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                spanning.resize(line_len, 0);
                read_range(&mut file, line_start, &mut spanning)?;
                &spanning
            };
            let line = ReverseLine {
                file: &mut file,
                start: line_start,
                raw,
                number: &mut number,
            };
            if let ControlFlow::Break(value) = visit(line) {
                return Ok(Some(value));
            }
            if let Some(number) = &mut number {
                *number -= 1;
            }
            line_end = line_start;
        }
    }
    Ok(None)
}

fn read_range(file: &mut File, start: u64, buffer: &mut [u8]) -> io::Result<()> {
    file.seek(SeekFrom::Start(start))?;
    file.read_exact(buffer)
}

pub struct ReverseLine<'a> {
    file: &'a mut File,
    start: u64,
    raw: &'a [u8],
    number: &'a mut Option<u64>,
}

impl<'a> ReverseLine<'a> {
    /// Returns the line as stored, including its terminator when present.
    pub fn raw(&self) -> &'a [u8] {
        self.raw
    }

    /// Counts newlines in the prefix on the first request during a scan.
    pub fn number(&mut self) -> io::Result<u32> {
        if let Some(number) = *self.number {
            return Ok(number.min(u64::from(u32::MAX)) as u32);
        }
        self.file.rewind()?;
        let mut buffer = [0; DEFAULT_BUFFER_SIZE];
        let mut remaining = self.start;
        let mut number = 1;
        while remaining > 0 {
            let size = remaining.min(buffer.len() as u64) as usize;
            self.file.read_exact(&mut buffer[..size])?;
            number += newline_offsets(&buffer[..size]).count() as u64;
            remaining -= size as u64;
        }
        *self.number = Some(number);
        Ok(number.min(u64::from(u32::MAX)) as u32)
    }
}

#[derive(Clone, Copy)]
pub struct Line<'a> {
    pub index: usize,
    pub raw: &'a [u8],
}

impl<'a> Line<'a> {
    #[inline]
    pub fn bytes(self) -> &'a [u8] {
        self.raw.strip_suffix(b"\n").unwrap_or(self.raw)
    }

    pub fn unterminated(self) -> bool {
        !self.raw.ends_with(b"\n")
    }

    pub fn number(self) -> u32 {
        Skip::line_number(self.index)
    }

    pub fn selected(self, prefilter: Option<&LinePrefilter>) -> bool {
        !self
            .bytes()
            .iter()
            .all(|byte| matches!(*byte, b' ' | b'\t' | b'\r'))
            && prefilter.is_none_or(|prefilter| prefilter.matches(self.bytes()))
    }

    #[inline]
    pub fn decode<T: Deserialize<'a>>(self) -> serde_json::Result<T> {
        parse(self.bytes())
    }

    pub fn record<T: Deserialize<'a>>(
        self,
        path: &Path,
        diagnostics: &Diagnostics,
        is_usage: Option<fn(&serde_json::Value) -> bool>,
    ) -> Option<T> {
        match self.decode() {
            Ok(record) => Some(record),
            Err(error) => {
                record_decode_error(
                    self.bytes(),
                    error,
                    path,
                    self.number(),
                    self.unterminated(),
                    diagnostics,
                    is_usage,
                );
                None
            }
        }
    }
}

#[inline]
pub fn parse<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> serde_json::Result<T> {
    serde_json::from_slice(bytes)
}

pub fn read_records<T: DeserializeOwned>(
    path: &Path,
    prefilter: Option<&LinePrefilter>,
    diagnostics: &Diagnostics,
    is_usage: fn(&serde_json::Value) -> bool,
    mut fold: impl FnMut(u32, T, &Diagnostics),
) -> Result<(), UnitError> {
    read_file(path, diagnostics, |line, local| {
        if line.selected(prefilter)
            && let Some(record) = line.record(path, local, Some(is_usage))
        {
            fold(line.number(), record, local);
        }
    })
}

pub fn read_file(
    path: &Path,
    diagnostics: &Diagnostics,
    mut fold: impl FnMut(Line<'_>, &Diagnostics),
) -> Result<(), UnitError> {
    read(
        path,
        Some(diagnostics),
        ReadPolicy {
            stage_diagnostics: true,
            ..ReadPolicy::PLAIN
        },
        |line, _, local| {
            if let Some(local) = local {
                fold(line, local);
            }
            ControlFlow::<()>::Continue(())
        },
    )?;
    Ok(())
}

pub fn decode_record<T: DeserializeOwned>(
    content: &[u8],
    unit: &Path,
    number: u32,
    row_id: Option<&str>,
    diagnostics: &Diagnostics,
    is_usage: Option<fn(&serde_json::Value) -> bool>,
) -> Option<T> {
    match serde_json::from_slice(content) {
        Ok(record) => Some(record),
        Err(error) => {
            record_decode_error(
                content,
                error,
                unit,
                number,
                false,
                &diagnostics.for_row(row_id),
                is_usage,
            );
            None
        }
    }
}

/// Reports the decoding category and column. Raw record text can include prompt content.
#[cold]
pub fn record_decode_error(
    record: &[u8],
    error: serde_json::Error,
    unit: &Path,
    number: u32,
    unterminated: bool,
    diagnostics: &Diagnostics,
    is_usage: Option<fn(&serde_json::Value) -> bool>,
) {
    use serde_json::error::Category;
    let (kind, category) = match error.classify() {
        Category::Eof if unterminated && being_written(unit, diagnostics.report_now()) => {
            diagnostics.debug_note(
                unit,
                format!(
                    "{}: {}:{number}: last line is incomplete, read as still being written",
                    diagnostics.source(),
                    unit.display()
                ),
            );
            return;
        }
        Category::Eof if unterminated => {
            diagnostics.record(Skip::record(
                unit,
                number,
                SkipKind::InvalidData,
                format!("last line is truncated at column {}", error.column()),
            ));
            return;
        }
        Category::Io => (SkipKind::Unreadable, "I/O"),
        Category::Syntax => (SkipKind::InvalidData, "syntax"),
        Category::Eof => (SkipKind::InvalidData, "EOF"),
        Category::Data => {
            let confirmed = is_usage.is_some_and(|confirm| {
                serde_json::from_slice::<serde_json::Value>(record)
                    .is_ok_and(|value| confirm(&value))
            });
            (
                if confirmed {
                    SkipKind::InvalidData
                } else {
                    SkipKind::UndecodedUsage
                },
                "data",
            )
        }
    };
    diagnostics.record(Skip::record(
        unit,
        number,
        kind,
        format!("JSON {category} error at column {}", error.column()),
    ));
}

pub fn lenient_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value
        .as_ref()
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default())
}

pub fn lenient_i64<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.as_ref().and_then(serde_json::Value::as_i64))
}

pub fn lenient_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.as_ref().and_then(serde_json::Value::as_f64))
}

pub fn lenient_object<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(value @ serde_json::Value::Object(_)) => serde_json::from_value(value).ok(),
        _ => None,
    })
}

pub fn lenient_array<'de, D, T>(deserializer: D) -> Result<Option<Vec<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(serde_json::Value::Array(items)) => Some(
            items
                .into_iter()
                .filter_map(|item| serde_json::from_value(item).ok())
                .collect(),
        ),
        _ => None,
    })
}

pub fn lenient_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    Ok(lenient_array(deserializer)?.unwrap_or_default())
}

pub fn non_empty_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(ccusage_core::non_empty_json_string(value.as_ref()))
}

macro_rules! skip_scalars {
    ($empty:expr) => {
        fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
            Ok($empty)
        }
        fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
            Ok($empty)
        }
        fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
            Ok($empty)
        }
        fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
            Ok($empty)
        }
        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok($empty)
        }
    };
}

pub trait Present {
    fn is_present(&self) -> bool;
}

pub trait FieldName {
    const NAME: &'static str;
}

/// Reads the first usable value of a field. Use lenient types for `T`, such as
/// [`LenientString`] or [`LenientSeq`], because Serde cannot recover from an
/// error midway through a streamed object.
pub struct FirstField<N, T>(pub Option<T>, PhantomData<N>);

impl<N, T> Default for FirstField<N, T> {
    fn default() -> Self {
        Self(None, PhantomData)
    }
}

impl<N, T: Present> Present for FirstField<N, T> {
    fn is_present(&self) -> bool {
        self.0.as_ref().is_some_and(Present::is_present)
    }
}

struct IsField<N>(bool, PhantomData<N>);

impl<'de, N: FieldName> Deserialize<'de> for IsField<N> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct KeyVisitor<N>(PhantomData<N>);

        impl<N: FieldName> Visitor<'_> for KeyVisitor<N> {
            type Value = IsField<N>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object key")
            }

            fn visit_str<E: de::Error>(self, key: &str) -> Result<Self::Value, E> {
                Ok(IsField(key == N::NAME, PhantomData))
            }
        }

        deserializer.deserialize_str(KeyVisitor(PhantomData))
    }
}

impl<'de, N: FieldName, T: Deserialize<'de> + Present> Deserialize<'de> for FirstField<N, T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FieldVisitor<N, T>(PhantomData<(N, T)>);

        impl<'de, N: FieldName, T: Deserialize<'de> + Present> Visitor<'de> for FieldVisitor<N, T> {
            type Value = FirstField<N, T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("any JSON value")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                std::iter::from_fn(|| {
                    map.next_key::<IsField<N>>()
                        .and_then(|key| {
                            key.map(|IsField(matches, _)| {
                                if matches {
                                    map.next_value::<T>().map(Some)
                                } else {
                                    map.next_value::<IgnoredAny>().map(|_| None)
                                }
                            })
                            .transpose()
                        })
                        .transpose()
                })
                .try_fold(None, |found: Option<T>, value| {
                    value.map(|value| found.or(value.filter(Present::is_present)))
                })
                .map(|found| FirstField(found, PhantomData))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
                IgnoredAny.visit_seq(seq).map(|_| FirstField::default())
            }

            fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Ok(FirstField::default())
            }

            skip_scalars!(FirstField::default());
        }

        deserializer.deserialize_any(FieldVisitor(PhantomData))
    }
}

#[derive(Default)]
pub struct LenientString(pub Option<String>);

impl Present for LenientString {
    fn is_present(&self) -> bool {
        self.0.is_some()
    }
}

impl<'de> Deserialize<'de> for LenientString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct StringVisitor;

        impl<'de> Visitor<'de> for StringVisitor {
            type Value = LenientString;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("any JSON value")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(LenientString(Some(value.to_owned())))
            }

            fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(LenientString(Some(value)))
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                IgnoredAny.visit_map(map).map(|_| LenientString(None))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
                IgnoredAny.visit_seq(seq).map(|_| LenientString(None))
            }

            skip_scalars!(LenientString(None));
        }

        deserializer.deserialize_any(StringVisitor)
    }
}

pub struct LenientSeq<T>(pub Vec<T>);

impl<T> Present for LenientSeq<T> {
    fn is_present(&self) -> bool {
        !self.0.is_empty()
    }
}

impl<T> Default for LenientSeq<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for LenientSeq<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SeqVisitor<T>(PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for SeqVisitor<T> {
            type Value = LenientSeq<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("any JSON value")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                std::iter::from_fn(|| seq.next_element().transpose())
                    .collect::<Result<_, _>>()
                    .map(LenientSeq)
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                IgnoredAny.visit_map(map).map(|_| LenientSeq(Vec::new()))
            }

            fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Ok(LenientSeq(Vec::new()))
            }

            skip_scalars!(LenientSeq(Vec::new()));
        }

        deserializer.deserialize_any(SeqVisitor(PhantomData))
    }
}

#[cfg(test)]
mod tests {
    use std::{ops::ControlFlow, path::Path};

    use serde::{Deserialize, de::DeserializeOwned};

    use ccusage_core::{Diagnostics, SkipKind, cli::SharedArgs};
    use ccusage_test_support::{Environment, Fixture};

    use crate::units::tests::age_past_write_window;

    use super::{
        FieldName, FirstField, LenientSeq, LenientString, ReadPolicy, lenient_array, lenient_f64,
        lenient_i64, lenient_object, lenient_u64, lenient_vec, non_empty_string, read,
    };
    use ccusage_core::fast::LinePrefilter;

    fn decode_fixture<T: DeserializeOwned>(
        bytes: &[u8],
        prefilter: Option<&LinePrefilter>,
        path: &Path,
        diagnostics: &Diagnostics,
        is_usage: Option<fn(&serde_json::Value) -> bool>,
    ) -> impl Iterator<Item = (u32, T)> {
        let mut output = Vec::new();
        let fixture = Fixture::new();
        let file = fixture.path("input.jsonl");
        std::fs::write(&file, bytes).unwrap();
        read(
            &file,
            Some(diagnostics),
            ReadPolicy {
                capacity: 7,
                ..ReadPolicy::PLAIN
            },
            |line, _, _| {
                if line.selected(prefilter)
                    && let Some(record) = line.record(path, diagnostics, is_usage)
                {
                    output.push((line.number(), record));
                }
                ControlFlow::<()>::Continue(())
            },
        )
        .unwrap();
        output.into_iter()
    }

    #[test]
    fn an_interruption_can_keep_the_buffered_unterminated_line() {
        let fixture = Fixture::new();
        let path = fixture.path("interrupted.jsonl.zst");
        let mut compressed = zstd::stream::encode_all(b"complete\npartial".as_slice(), 0).unwrap();
        compressed.extend_from_slice(b"invalid next frame");
        std::fs::write(&path, compressed).unwrap();
        let diagnostics = Diagnostics::new("test");
        let mut lines = Vec::new();
        read(
            &path,
            Some(&diagnostics),
            ReadPolicy {
                compressed: true,
                keep_interrupted_tail: |_| true,
                interrupted: |_| {
                    super::Interruption::KeepRecords(SkipKind::ReadInterrupted, "file read failed")
                },
                ..ReadPolicy::PLAIN
            },
            |line, _, _| {
                lines.push((line.index, line.bytes().to_vec(), line.unterminated()));
                ControlFlow::<()>::Continue(())
            },
        )
        .unwrap();

        assert_eq!(
            lines,
            [
                (0, b"complete".to_vec(), false),
                (1, b"partial".to_vec(), true)
            ]
        );
        assert_eq!(diagnostics.skipped(SkipKind::ReadInterrupted), 1);
    }

    #[derive(Debug, PartialEq, Deserialize)]
    struct Record {
        #[serde(default, deserialize_with = "non_empty_string")]
        model: Option<String>,
        #[serde(default, deserialize_with = "lenient_u64")]
        tokens: u64,
    }

    #[test]
    fn records_skips_lines_without_marker() {
        let content =
            b"{\"model\":\"a\",\"tokens\":1}\n{\"other\":true}\n{\"model\":\"b\",\"tokens\":2}\n";
        let prefilter = LinePrefilter::all(&[b"model"]);
        let diagnostics = Diagnostics::new("test");
        let parsed = decode_fixture::<Record>(
            content,
            Some(&prefilter),
            Path::new("/logs/session.jsonl"),
            &diagnostics,
            None,
        )
        .map(|(_, record)| record)
        .collect::<Vec<_>>();
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 0);

        assert_eq!(
            parsed,
            [
                Record {
                    model: Some("a".to_string()),
                    tokens: 1,
                },
                Record {
                    model: Some("b".to_string()),
                    tokens: 2,
                },
            ]
        );
    }

    const CUT_LAST_LINE: &str = "{\"tokens\":1}\n{\"tokens\":2,\"mod";

    #[test]
    fn a_record_cut_short_by_the_end_of_a_file_being_written_is_noted_not_skipped() {
        let fixture = Fixture::new();
        let live = fixture.write_file("live.jsonl", CUT_LAST_LINE);
        let diagnostics = Diagnostics::new("test");
        let records =
            decode_fixture::<Record>(CUT_LAST_LINE.as_bytes(), None, &live, &diagnostics, None)
                .map(|(_, record)| record.tokens)
                .collect::<Vec<_>>();

        assert_eq!(records, [1]);
        Environment::with_var("LOG_LEVEL", "1").run(|| {
            assert!(diagnostics.warnings(&SharedArgs::default()).is_empty());
            assert_eq!(
                diagnostics.warnings(&SharedArgs {
                    debug: true,
                    ..SharedArgs::default()
                }),
                [format!(
                    "test: {}:2: last line is incomplete, read as still being written",
                    live.display()
                )]
            );
        })
    }

    #[test]
    fn a_record_cut_short_by_the_end_of_a_file_left_unchanged_is_skipped_as_truncated() {
        let fixture = Fixture::new();
        let stale = fixture.write_file("stale.jsonl", CUT_LAST_LINE);
        age_past_write_window(&stale);
        let diagnostics = Diagnostics::new("test");
        let records =
            decode_fixture::<Record>(CUT_LAST_LINE.as_bytes(), None, &stale, &diagnostics, None)
                .map(|(_, record)| record.tokens)
                .collect::<Vec<_>>();

        assert_eq!(records, [1]);
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        Environment::with_var("LOG_LEVEL", "1").run(|| {
            let warnings = diagnostics
                .warnings(&SharedArgs {
                    debug: true,
                    ..SharedArgs::default()
                })
                .join("\n");
            assert!(
                warnings.contains(&format!(
                    "{}:2: last line is truncated at column 16",
                    stale.display()
                )),
                "{warnings}"
            );
            assert!(!warnings.contains("still being written"), "{warnings}");
        })
    }

    #[test]
    fn a_record_cut_short_before_a_newline_is_invalid_data() {
        for content in [
            b"{\"tokens\":2,\"mod\n{\"tokens\":1}".as_slice(),
            b"{\"tokens\":1}\n{\"tokens\":2,\"mod\n",
        ] {
            let diagnostics = Diagnostics::new("test");
            let _ = decode_fixture::<Record>(
                content,
                None,
                Path::new("/logs/torn.jsonl"),
                &diagnostics,
                None,
            )
            .count();

            assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        }
    }

    #[test]
    fn records_skips_unparsable_lines() {
        let content = b"{\"tokens\":1}\nnot json\n{\"tokens\":2}\n";
        let diagnostics = Diagnostics::new("test");
        let parsed = decode_fixture::<Record>(
            content,
            None,
            Path::new("/logs/session.jsonl"),
            &diagnostics,
            None,
        )
        .map(|(_, record)| record)
        .collect::<Vec<_>>();

        assert_eq!(
            parsed
                .iter()
                .map(|record| record.tokens)
                .collect::<Vec<_>>(),
            [1, 2]
        );
    }

    #[test]
    fn records_with_diagnostics_skips_bad_selected_lines_without_leaking_input() {
        #[derive(Debug, Deserialize)]
        struct StrictRecord {
            model: String,
            tokens: u64,
        }

        let content = b"{\"model\":\"before\",\"tokens\":1}\n{\"other\":\"SECRET_CANARY\",\"tokens\":}\n{\"model\":\"broken\",\"tokens\":\"SECRET_CANARY\"}\n{\"model\":\"after\",\"tokens\":2}";
        let prefilter = LinePrefilter::all(&[b"model"]);
        let diagnostics = Diagnostics::new("test");

        let records = decode_fixture::<StrictRecord>(
            content,
            Some(&prefilter),
            Path::new("/logs/session.jsonl"),
            &diagnostics,
            None,
        )
        .map(|(line, record)| (line, record.model, record.tokens))
        .collect::<Vec<_>>();

        assert_eq!(
            records,
            [(1, "before".to_string(), 1), (4, "after".to_string(), 2)]
        );
        assert_eq!(diagnostics.skipped(SkipKind::UndecodedUsage), 1);

        Environment::with_var("LOG_LEVEL", "1").run(|| {
            let warnings = diagnostics
                .warnings(&SharedArgs {
                    debug: true,
                    ..SharedArgs::default()
                })
                .join("\n");
            assert!(warnings.contains("/logs/session.jsonl:3: JSON data error at column"));
            assert!(!warnings.contains("SECRET_CANARY"));
        })
    }

    #[test]
    fn confirmed_usage_decode_failures_warn_without_labeling_other_records_as_usage() {
        #[derive(Deserialize)]
        struct Record {
            tokens: u64,
        }
        let diagnostics = Diagnostics::new("test");
        let records = decode_fixture::<Record>(
            b"{\"type\":\"user\",\"tokens\":\"text\"}\n{\"type\":\"usage\",\"tokens\":\"SECRET_CANARY\"}\n{\"type\":\"usage\",\"tokens\":3}",
            None,
            Path::new("/logs/session.jsonl"),
            &diagnostics,
            Some(|value| value.get("type").and_then(serde_json::Value::as_str) == Some("usage")),
        ).map(|(_, record)| record.tokens).collect::<Vec<_>>();

        assert_eq!(records, [3]);
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        assert_eq!(diagnostics.skipped(SkipKind::UndecodedUsage), 1);
    }

    #[test]
    fn ignores_blank_jsonl_lines_and_keeps_physical_line_numbers() {
        #[derive(Debug, Deserialize)]
        struct UsageRecord {
            tokens: u64,
        }

        let content = concat!(
            r#"{"kind":"usage","tokens":1}"#,
            "\n \t\r\n",
            r#"{"kind":"usage","tokens":"SECRET_CANARY"}"#,
            "\n",
            r#"{"kind":"usage","tokens":2}"#,
        )
        .as_bytes();
        let diagnostics = Diagnostics::new("test");
        let records = decode_fixture::<UsageRecord>(
            content,
            None,
            Path::new("/logs/session.jsonl"),
            &diagnostics,
            Some(|value| value.get("kind").and_then(serde_json::Value::as_str) == Some("usage")),
        )
        .map(|(_, record)| record.tokens)
        .collect::<Vec<_>>();

        assert_eq!(records, [1, 2]);
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);

        Environment::with_var("LOG_LEVEL", "1").run(|| {
            let warnings = diagnostics
                .warnings(&SharedArgs {
                    debug: true,
                    ..SharedArgs::default()
                })
                .join("\n");
            assert!(warnings.contains("skipped 1 record in 1 input (invalid data)"));
            assert!(warnings.contains("/logs/session.jsonl:3: JSON data error"));
            assert!(!warnings.contains("/logs/session.jsonl:2:"));
            assert!(!warnings.contains("SECRET_CANARY"));

            let database_diagnostics = Diagnostics::new("test");
            assert!(
                super::decode_record::<UsageRecord>(
                    b" \t\r",
                    Path::new("/logs/state.db"),
                    1,
                    None,
                    &database_diagnostics,
                    Some(|_| true),
                )
                .is_none()
            );
            assert_eq!(
                database_diagnostics.skipped(SkipKind::InvalidData),
                1,
                "blank database cells are not JSONL blank lines"
            );
        })
    }

    #[test]
    fn lenient_wrappers_skip_values_of_another_shape_and_repeated_fields() {
        struct Id;
        impl FieldName for Id {
            const NAME: &'static str = "id";
        }
        struct Child;
        impl FieldName for Child {
            const NAME: &'static str = "child";
        }
        struct Parts;
        impl FieldName for Parts {
            const NAME: &'static str = "parts";
        }
        type ChildId = FirstField<Child, FirstField<Id, LenientString>>;

        let parse = |raw: &str| {
            serde_json::from_str::<FirstField<Parts, LenientSeq<ChildId>>>(raw)
                .unwrap()
                .0
                .map(|parts| {
                    parts
                        .0
                        .into_iter()
                        .map(|part| part.0.and_then(|child| child.0).and_then(|id| id.0))
                        .collect::<Vec<_>>()
                })
        };

        assert_eq!(
            parse(
                r#"{"parts":[{"child":{"id":"x"}},"junk",{"child":[1]},{"child":{"id":{"o":[2]}}},7,null,{"child":{"id":["n"],"id":"first","id":"second"},"child":{"id":"later"}},{"child":{"id":5},"child":{"id":"y"}}]}"#
            ),
            Some(vec![
                Some("x".to_string()),
                None,
                None,
                None,
                None,
                None,
                Some("first".to_string()),
                Some("y".to_string()),
            ])
        );
        assert_eq!(
            parse(r#"{"parts":"x","parts":[{"child":{"id":"z"}}]}"#),
            Some(vec![Some("z".to_string())])
        );
        assert_eq!(parse(r#"{"parts":{"a":[1]}}"#), None);
        assert_eq!(parse(r#"["parts"]"#), None);
    }

    #[test]
    fn lenient_u64_matches_value_as_u64() {
        let coerce = |raw: &str| {
            serde_json::from_str::<Record>(&format!("{{\"tokens\":{raw}}}"))
                .unwrap()
                .tokens
        };

        assert_eq!(coerce("42"), 42);
        assert_eq!(coerce("12.5"), 0);
        assert_eq!(coerce("-1"), 0);
        assert_eq!(coerce("\"7\""), 0);
        assert_eq!(coerce("null"), 0);
    }

    #[test]
    fn lenient_i64_and_f64_match_value_accessors() {
        #[derive(Deserialize)]
        struct Numbers {
            #[serde(default, deserialize_with = "lenient_i64")]
            created: Option<i64>,
            #[serde(default, deserialize_with = "lenient_f64")]
            cost: Option<f64>,
        }

        let parse = |raw: &str| serde_json::from_str::<Numbers>(raw).unwrap();

        let both = parse("{\"created\":-5,\"cost\":1.5}");
        assert_eq!(both.created, Some(-5));
        assert_eq!(both.cost, Some(1.5));

        let mixed = parse("{\"created\":1.5,\"cost\":7}");
        assert_eq!(mixed.created, None);
        assert_eq!(mixed.cost, Some(7.0));

        let strings = parse("{\"created\":\"3\",\"cost\":\"x\"}");
        assert_eq!(strings.created, None);
        assert_eq!(strings.cost, None);

        let missing = parse("{}");
        assert_eq!(missing.created, None);
        assert_eq!(missing.cost, None);
    }

    #[test]
    fn lenient_object_keeps_record_when_nested_field_is_not_an_object() {
        #[derive(Debug, PartialEq, Deserialize)]
        struct Nested {
            #[serde(default, deserialize_with = "lenient_u64")]
            read: u64,
        }

        #[derive(Debug, PartialEq, Deserialize)]
        struct Outer {
            #[serde(default, deserialize_with = "lenient_object")]
            cache: Option<Nested>,
            #[serde(default, deserialize_with = "lenient_u64")]
            input: u64,
        }

        let parse = |raw: &str| serde_json::from_str::<Outer>(raw).unwrap();

        assert_eq!(
            parse("{\"cache\":{\"read\":5},\"input\":7}"),
            Outer {
                cache: Some(Nested { read: 5 }),
                input: 7,
            }
        );

        for raw in [
            "{\"cache\":5,\"input\":7}",
            "{\"cache\":\"oops\",\"input\":7}",
            "{\"cache\":[1,2],\"input\":7}",
            "{\"cache\":null,\"input\":7}",
            "{\"input\":7}",
        ] {
            assert_eq!(
                parse(raw),
                Outer {
                    cache: None,
                    input: 7,
                },
                "raw: {raw}"
            );
        }
    }

    #[test]
    fn lenient_array_and_vec_skip_bad_elements_and_tolerate_non_arrays() {
        #[derive(Debug, PartialEq, Deserialize)]
        struct Item {
            #[serde(default, deserialize_with = "lenient_u64")]
            value: u64,
        }

        #[derive(Debug, PartialEq, Deserialize)]
        struct Outer {
            #[serde(default, deserialize_with = "lenient_array")]
            optional: Option<Vec<Item>>,
            #[serde(default, deserialize_with = "lenient_vec")]
            required: Vec<Item>,
        }

        let parse = |raw: &str| serde_json::from_str::<Outer>(raw).unwrap();

        let mixed =
            parse(r#"{"optional":[{"value":1},"oops",{"value":2}],"required":[3,{"value":4}]}"#);
        assert_eq!(
            mixed.optional,
            Some(vec![Item { value: 1 }, Item { value: 2 }])
        );
        assert_eq!(mixed.required, vec![Item { value: 4 }]);

        let empty = parse(r#"{"optional":[],"required":[]}"#);
        assert_eq!(empty.optional, Some(vec![]));
        assert_eq!(empty.required, vec![]);

        for raw in [
            r#"{"optional":5,"required":"nope"}"#,
            r#"{"optional":null,"required":null}"#,
            r#"{}"#,
        ] {
            let parsed = parse(raw);
            assert_eq!(parsed.optional, None, "raw: {raw}");
            assert_eq!(parsed.required, Vec::<Item>::new(), "raw: {raw}");
        }
    }

    #[test]
    fn non_empty_string_trims_and_drops_empty() {
        let parse = |raw: &str| {
            serde_json::from_str::<Record>(&format!("{{\"model\":{raw}}}"))
                .unwrap()
                .model
        };

        assert_eq!(parse("\"  qwen  \""), Some("qwen".to_string()));
        assert_eq!(parse("\"   \""), None);
        assert_eq!(parse("123"), None);
        assert_eq!(parse("null"), None);
    }
}
