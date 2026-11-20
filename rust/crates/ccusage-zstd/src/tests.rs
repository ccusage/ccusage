use std::{
    fs::File,
    io::{self, BufReader},
    path::Path,
};

use ccusage_core::fast::LineReader;
use ccusage_test_support::Fixture;
use zstd::zstd_safe::CParameter;

use super::ZstdLines;

#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    lines: Vec<Vec<u8>>,
    error: Option<(io::ErrorKind, String)>,
    rest: Vec<u8>,
}

fn reference(path: &Path, capacity: usize) -> Outcome {
    let file = File::open(path).unwrap();
    let decoder =
        zstd::stream::read::Decoder::with_buffer(BufReader::with_capacity(capacity, file)).unwrap();
    let mut reader = LineReader::with_buffer(vec![0; capacity], decoder);
    let mut lines = Vec::new();
    let error = loop {
        match reader.next_line() {
            Ok(Some(line)) => lines.push(line.to_vec()),
            Ok(None) => break None,
            Err(error) => break Some((error.kind(), error.to_string())),
        }
    };
    Outcome {
        lines,
        error,
        rest: reader.rest().to_vec(),
    }
}

fn kind(error: &Option<(io::ErrorKind, String)>) -> Option<io::ErrorKind> {
    error.as_ref().map(|(kind, _)| *kind)
}

fn direct(path: &Path, capacity: usize, fail_after: Option<usize>) -> (Outcome, bool) {
    let mut reader = ZstdLines::new(File::open(path).unwrap(), capacity).unwrap();
    if let super::Inner::Direct(direct) = &mut reader.inner {
        direct.fail_after = fail_after;
    }
    let mut lines = Vec::new();
    let error = loop {
        match reader.next_line() {
            Ok(Some(line)) => lines.push(line.to_vec()),
            Ok(None) => break None,
            Err(error) => break Some((error.kind(), error.to_string())),
        }
    };
    let outcome = Outcome {
        lines,
        error,
        rest: reader.rest().to_vec(),
    };
    (outcome, reader.replaying())
}

struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

fn text(random: &mut Random, lines: usize, long_every: usize) -> Vec<u8> {
    let mut text = Vec::new();
    for index in 0..lines {
        let length = if long_every != 0 && index % long_every == long_every - 1 {
            200_000 + random.below(400_000)
        } else {
            random.below(2_000)
        };
        let seed = random.next();
        text.extend((0..length).map(|offset| {
            let byte = (seed.wrapping_add(offset as u64 / 7) % 23) as u8;
            if byte == 0 { b'\n' } else { b'a' + byte }
        }));
        text.push(b'\n');
    }
    text
}

fn compress(data: &[u8], level: i32, window_log: Option<u32>, content_size: bool) -> Vec<u8> {
    let mut compressor = zstd::bulk::Compressor::new(level).unwrap();
    if let Some(log) = window_log {
        compressor
            .set_parameter(CParameter::WindowLog(log))
            .unwrap();
    }
    compressor
        .set_parameter(CParameter::ContentSizeFlag(content_size))
        .unwrap();
    compressor
        .set_parameter(CParameter::ChecksumFlag(true))
        .unwrap();
    compressor.compress(data).unwrap()
}

fn skippable(payload: &[u8]) -> Vec<u8> {
    let mut frame = 0x184D_2A53_u32.to_le_bytes().to_vec();
    frame.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
    frame.extend_from_slice(payload);
    frame
}

fn streams(random: &mut Random) -> Vec<Vec<u8>> {
    let mut streams = vec![
        Vec::new(),
        skippable(b"only metadata"),
        compress(b"", 3, None, true),
        compress(b"complete\npartial", 3, None, true),
    ];
    for (lines, long_every, level, window_log, content_size) in [
        (40, 0, 3, None, true),
        (400, 0, 1, Some(10), true),
        (400, 0, 3, Some(14), false),
        (3_000, 0, 1, Some(17), false),
        (60, 7, 3, Some(17), true),
        (60, 5, 1, Some(18), false),
        (2_000, 0, 19, None, true),
    ] {
        let data = text(random, lines, long_every);
        streams.push(compress(&data, level, window_log, content_size));
        let split = random.below(data.len());
        let mut frames = compress(&data[..split], level, window_log, content_size);
        frames.extend(skippable(&data[..random.below(64)]));
        frames.extend(compress(&data[split..], level, window_log, !content_size));
        streams.push(frames);
    }
    streams
}

fn write(fixture: &Fixture, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = fixture.path(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

const CAPACITIES: [usize; 3] = [61, 8 * 1024, 128 * 1024];

#[test]
fn whole_and_truncated_streams_decode_like_the_streaming_decoder() {
    let fixture = Fixture::new();
    let mut random = Random(0x9E37_79B9_7F4A_7C15);
    for (index, stream) in streams(&mut random).into_iter().enumerate() {
        let mut cuts = vec![stream.len()];
        cuts.extend((0..12).map(|_| random.below(stream.len().max(1))));
        cuts.extend(
            [1, 4, 5, 9, 17, 18]
                .into_iter()
                .filter(|cut| *cut < stream.len()),
        );
        for cut in cuts {
            let path = write(&fixture, "stream.zst", &stream[..cut]);
            for capacity in CAPACITIES {
                let expected = reference(&path, capacity);
                let (actual, replaying) = direct(&path, capacity, None);
                assert_eq!(
                    actual, expected,
                    "stream {index} cut {cut} capacity {capacity}"
                );
                assert!(!replaying, "stream {index} cut {cut} capacity {capacity}");
            }
        }
    }
}

#[test]
fn corrupted_and_trailing_bytes_fail_like_the_streaming_decoder() {
    let fixture = Fixture::new();
    let mut random = Random(0xD1B5_4A32_D192_ED03);
    for (index, stream) in streams(&mut random).into_iter().enumerate() {
        for round in 0..24 {
            let mut corrupted = stream.clone();
            if round % 4 == 0 {
                corrupted.extend_from_slice(b"invalid next frame");
            } else if !corrupted.is_empty() {
                for _ in 0..=random.below(3) {
                    let at = random.below(corrupted.len());
                    corrupted[at] ^= 1 << random.below(8);
                }
            }
            let path = write(&fixture, "corrupt.zst", &corrupted);
            let references = CAPACITIES.map(|capacity| reference(&path, capacity));
            let (actual, _) = direct(&path, CAPACITIES[0], None);
            let context = format!("stream {index} round {round}");
            for capacity in &CAPACITIES[1..] {
                assert_eq!(
                    direct(&path, *capacity, None).0,
                    actual,
                    "{context} {capacity}"
                );
            }
            // NOTE(2026-11-20): The streaming decoder decodes a small frame in one pass when its input buffer holds all of it, so its verdict on a damaged frame depends on the buffer size. Which check names the damage differs too, and callers read only the kind.
            let verdicts = references.each_ref().map(|outcome| kind(&outcome.error));
            if verdicts.iter().all(|verdict| *verdict == verdicts[0]) {
                assert_eq!(kind(&actual.error), verdicts[0], "{context}");
            } else {
                assert!(verdicts.contains(&kind(&actual.error)), "{context}");
            }
            for expected in &references {
                // NOTE(2026-11-20): Where a damaged frame stops depends on how much of it each decoder holds at once. The lines both return before stopping must still agree.
                let shared = actual.lines.len().min(expected.lines.len());
                assert_eq!(
                    actual.lines[..shared],
                    expected.lines[..shared],
                    "{context}"
                );
            }
        }
    }
}

#[test]
fn an_unexpected_decoder_state_continues_on_the_streaming_decoder() {
    let fixture = Fixture::new();
    let mut random = Random(0x2545_F491_4F6C_DD1D);
    for (index, stream) in streams(&mut random).into_iter().enumerate() {
        let path = write(&fixture, "fallback.zst", &stream);
        for fail_after in [0, 1, 3, 40] {
            let expected = reference(&path, 8 * 1024);
            let (actual, _) = direct(&path, 8 * 1024, Some(fail_after));
            assert_eq!(actual, expected, "stream {index} after {fail_after} calls");
        }
    }
}

#[test]
fn a_line_longer_than_the_window_survives_the_ring_wrap() {
    let fixture = Fixture::new();
    let mut data = Vec::new();
    for index in 0..40_u32 {
        let length = if index % 9 == 8 { 3 << 20 } else { 70_000 };
        data.extend((0..length).map(|offset| b'a' + ((offset / 5 + index) % 26) as u8));
        data.push(b'\n');
    }
    let path = write(&fixture, "long.zst", &compress(&data, 1, Some(17), false));
    let (actual, replaying) = direct(&path, 128 * 1024, None);
    assert_eq!(actual, reference(&path, 128 * 1024));
    assert_eq!(actual.lines.concat(), data);
    assert!(!replaying);
}

#[test]
fn a_frame_needing_more_than_the_window_limit_is_refused_like_the_streaming_decoder() {
    let fixture = Fixture::new();
    let mut frame = 0xFD2F_B528_u32.to_le_bytes().to_vec();
    frame.extend_from_slice(&[0x00, 18 << 3, 0x01, 0x00, 0x00]);
    let path = write(&fixture, "window.zst", &frame);
    let expected = reference(&path, 8 * 1024);
    assert!(expected.error.is_some());
    assert_eq!(direct(&path, 8 * 1024, None).0, expected);
}
