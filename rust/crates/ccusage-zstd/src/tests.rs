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

#[test]
fn a_replay_that_ends_before_the_returned_lines_is_an_error() {
    let fixture = Fixture::new();
    let mut random = Random(0x0BAD_5EED);
    let mut stream = Vec::new();
    for _ in 0..4 {
        stream.extend(compress(&text(&mut random, 200, 0), 3, None, true));
    }
    let path = write(&fixture, "shrinking.zst", &stream);
    let mut reader = ZstdLines::new(File::open(&path).unwrap(), 8 * 1024).unwrap();
    for _ in 0..500 {
        reader.next_line().unwrap().unwrap();
    }
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let mut shorter = compress(b"one line\n", 3, None, true);
    shorter.extend(skippable(&vec![0; stream.len() - shorter.len() - 8]));
    assert_eq!(shorter.len(), stream.len());
    let writer = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    std::io::Write::write_all(&mut &writer, &shorter).unwrap();
    writer.set_modified(modified).unwrap();
    drop(writer);
    if let super::Inner::Direct(direct) = &mut reader.inner {
        direct.fail_after = Some(0);
    }
    let error = loop {
        match reader.next_line() {
            Ok(Some(_)) => {}
            Ok(None) => panic!("the replay must not end cleanly"),
            Err(error) => break error,
        }
    };
    assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    assert_eq!(
        error.to_string(),
        "zstd replay ended before the lines already returned"
    );
}

#[test]
fn a_frame_that_understates_its_window_decodes_at_least_what_the_streaming_decoder_recovers() {
    let fixture = Fixture::new();
    let mut random = Random(0x5EED_0001);
    let mut accepted_beyond_the_streaming_decoder = 0;
    for round in 0..60 {
        let lines = 300 + random.below(1500);
        let data = text(&mut random, lines, 0);
        let checksum = round % 2 == 0;
        let mut compressor = zstd::bulk::Compressor::new(3).unwrap();
        compressor.set_parameter(CParameter::WindowLog(20)).unwrap();
        compressor
            .set_parameter(CParameter::ContentSizeFlag(round % 3 != 0))
            .unwrap();
        compressor
            .set_parameter(CParameter::ChecksumFlag(checksum))
            .unwrap();
        let mut stream = compressor.compress(&data).unwrap();
        if stream[4] & 0x20 != 0 {
            continue;
        }
        stream[5] = (random.below(9) as u8) << 3;
        let path = write(&fixture, "understated.zst", &stream);
        for capacity in CAPACITIES {
            let expected = reference(&path, capacity);
            let (actual, _) = direct(&path, capacity, None);
            let context = format!("round {round} capacity {capacity}");
            if expected.error.is_none() && expected.lines.concat() == data {
                assert_eq!(actual, expected, "{context}");
            }
            if checksum && actual.error.is_none() {
                assert_eq!(actual.lines.concat(), data, "{context}");
            }
            if actual.error.is_none() && expected.error.is_some() {
                accepted_beyond_the_streaming_decoder += 1;
            }
        }
    }
    assert!(accepted_beyond_the_streaming_decoder > 0);
}

#[test]
fn a_decode_error_is_terminal() {
    let fixture = Fixture::new();
    let mut stream = vec![
        0x28, 0xB5, 0x2F, 0xFD, 0x24, 0x20, 0x01, 0x00, 0x00, 0x99, 0xE9, 0xD8, 0x51,
    ];
    stream.extend(compress(b"after\n", 3, None, true));
    let path = write(&fixture, "terminal.zst", &stream);
    let mut reader = ZstdLines::new(File::open(&path).unwrap(), 8 * 1024).unwrap();
    let error = reader.next_line().unwrap_err();
    for _ in 0..3 {
        let again = reader.next_line().unwrap_err();
        assert_eq!(
            (again.kind(), again.to_string()),
            (error.kind(), error.to_string())
        );
    }
}

#[test]
fn no_line_of_a_frame_that_fits_the_ring_is_returned_before_its_checksum_passes() {
    let fixture = Fixture::new();
    let data = text(&mut Random(0xC0FF_EE01), 40, 0);
    let mut stream = compress(&data, 3, None, true);
    let checksum = stream.len() - 1;
    stream[checksum] ^= 0x01;
    let path = write(&fixture, "checksum.zst", &stream);
    for capacity in CAPACITIES {
        let (actual, _) = direct(&path, capacity, None);
        assert_eq!(actual.lines, Vec::<Vec<u8>>::new(), "capacity {capacity}");
        assert_eq!(kind(&actual.error), Some(io::ErrorKind::Other));
        assert_eq!(actual.rest, Vec::<u8>::new(), "capacity {capacity}");
    }
}

#[test]
fn a_large_frame_returns_no_more_lines_before_a_bad_checksum_than_the_streaming_decoder() {
    let fixture = Fixture::new();
    let data = text(&mut Random(0xC0FF_EE02), 3_000, 0);
    let mut stream = compress(&data, 1, Some(17), false);
    let checksum = stream.len() - 1;
    stream[checksum] ^= 0x01;
    let path = write(&fixture, "large-checksum.zst", &stream);
    let expected = reference(&path, 128 * 1024);
    assert!(!expected.lines.is_empty());
    assert_eq!(
        kind(&expected.error),
        Some(io::ErrorKind::Other),
        "{:?}",
        expected.error
    );
    let actual = direct(&path, 128 * 1024, None).0;
    assert_eq!(actual.error, expected.error);
    assert!(!actual.lines.is_empty());
    assert!(
        expected.lines.starts_with(&actual.lines),
        "{} direct lines, {} streaming lines",
        actual.lines.len(),
        expected.lines.len()
    );
}

#[test]
fn a_single_segment_window_one_past_the_limit_is_accepted_like_the_streaming_decoder() {
    let fixture = Fixture::new();
    for (content, accepted) in [((128 << 20) + 1, true), ((128 << 20) + 2, false)] {
        let mut frame = 0xFD2F_B528_u32.to_le_bytes().to_vec();
        frame.push(0xE0);
        frame.extend_from_slice(&(content as u64).to_le_bytes());
        let path = write(&fixture, "window-limit.zst", &frame);
        let expected = reference(&path, 8 * 1024);
        assert_eq!(
            kind(&expected.error),
            Some(if accepted {
                io::ErrorKind::UnexpectedEof
            } else {
                io::ErrorKind::Other
            }),
            "content {content}: {:?}",
            expected.error
        );
        assert_eq!(
            direct(&path, 8 * 1024, None).0,
            expected,
            "content {content}"
        );
    }
}

#[test]
fn a_line_spanning_many_small_frames_grows_the_ring_geometrically() {
    let fixture = Fixture::new();
    let line: Vec<u8> = (0..4 << 20)
        .map(|offset| b'a' + (offset % 26) as u8)
        .collect();
    let mut stream = Vec::new();
    for chunk in line.chunks(8 * 1024) {
        stream.extend(compress(chunk, 1, None, true));
    }
    stream.extend(compress(b"\n", 1, None, true));
    let path = write(&fixture, "spanning.zst", &stream);
    let mut reader = ZstdLines::new(File::open(&path).unwrap(), 8 * 1024).unwrap();
    assert_eq!(reader.next_line().unwrap().unwrap().len(), line.len() + 1);
    let super::Inner::Direct(direct) = &reader.inner else {
        panic!("the direct decoder replayed");
    };
    assert!(direct.grows <= 16, "{} grows", direct.grows);
}

#[test]
fn a_replay_rejects_a_file_rewritten_in_place() {
    let fixture = Fixture::new();
    let mut random = Random(0x0BAD_F11E);
    let first = text(&mut random, 3_000, 0);
    let path = write(
        &fixture,
        "rewritten.zst",
        &compress(&first, 1, Some(17), false),
    );
    let file = File::open(&path).unwrap();
    let mut reader = ZstdLines::new(file, 8 * 1024).unwrap();
    for _ in 0..10 {
        reader.next_line().unwrap().unwrap();
    }
    let mut rewritten = first.clone();
    rewritten.reverse();
    let rewritten = compress(&rewritten, 1, Some(17), false);
    let mut writer = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&path)
        .unwrap();
    std::io::Write::write_all(&mut writer, &rewritten).unwrap();
    writer
        .set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(86_400))
        .unwrap();
    drop(writer);
    if let super::Inner::Direct(direct) = &mut reader.inner {
        direct.fail_after = Some(0);
    }
    let error = loop {
        match reader.next_line() {
            Ok(Some(_)) => {}
            Ok(None) => panic!("the replay resumed in a rewritten file"),
            Err(error) => break error,
        }
    };
    assert_eq!(
        error.to_string(),
        "zstd replay found the file changed since it was opened"
    );
}
