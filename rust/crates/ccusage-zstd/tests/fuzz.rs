use std::{
    fs::File,
    io::{self, BufReader, Read, Write},
    path::{Path, PathBuf},
};

use ccusage_test_support::Fixture;
use ccusage_zstd::ZstdLines;
use zstd::zstd_safe::CParameter;

#[derive(Debug, PartialEq, Eq, Default)]
struct Outcome {
    lines: Vec<Vec<u8>>,
    error: Option<(io::ErrorKind, String)>,
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
        if bound == 0 {
            0
        } else {
            (self.next() % bound as u64) as usize
        }
    }
}

fn reference(path: &Path) -> Outcome {
    let file = File::open(path).expect("fuzz input setup");
    let mut decoder =
        zstd::stream::read::Decoder::with_buffer(BufReader::new(file)).expect("fuzz input setup");
    let mut all = Vec::new();
    let mut chunk = vec![0; 65536];
    let error = loop {
        match decoder.read(&mut chunk) {
            Ok(0) => break None,
            Ok(n) => all.extend_from_slice(&chunk[..n]),
            Err(e) => break Some((e.kind(), e.to_string())),
        }
        if all.len() > 600 << 20 {
            break Some((io::ErrorKind::Other, "too big".into()));
        }
    };
    let mut lines: Vec<Vec<u8>> = all
        .split_inclusive(|b| *b == b'\n')
        .map(<[u8]>::to_vec)
        .collect();
    if error.is_some() && lines.last().is_some_and(|l| l.last() != Some(&b'\n')) {
        lines.pop();
    }
    Outcome { lines, error }
}

fn direct(path: &Path, capacity: usize) -> Outcome {
    if setting::<String>("FUZZ_FRESH").is_some() {
        return on_fresh_thread(path.to_path_buf(), capacity);
    }
    direct_inner(path, capacity)
}

#[expect(
    clippy::disallowed_methods,
    reason = "a fresh thread starts without recycled buffers; no fixture environment is involved"
)]
fn on_fresh_thread(path: PathBuf, capacity: usize) -> Outcome {
    std::thread::spawn(move || direct_inner(&path, capacity))
        .join()
        .expect("the fresh decoding thread panicked")
}

#[expect(
    clippy::disallowed_methods,
    reason = "the fuzz loop takes its size and seed from the environment of the test run"
)]
fn setting<T: std::str::FromStr>(name: &str) -> Option<T> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
}

fn direct_inner(path: &Path, capacity: usize) -> Outcome {
    let mut reader = ZstdLines::new(File::open(path).expect("fuzz input setup"), capacity)
        .expect("fuzz input setup");
    let mut lines = Vec::new();
    let mut total = 0usize;
    let error = loop {
        match reader.next_line() {
            Ok(Some(line)) => {
                total += line.len();
                lines.push(line.to_vec())
            }
            Ok(None) => break None,
            Err(e) => break Some((e.kind(), e.to_string())),
        }
        if total > 600 << 20 {
            break Some((io::ErrorKind::Other, "too big".into()));
        }
    };
    Outcome { lines, error }
}

fn data(random: &mut Random, size: usize, window: usize) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(size + 1024);
    while out.len() < size {
        match random.below(6) {
            0 => {
                let n = random.below(3000);
                out.extend_from_slice(b"{\"type\":\"event\",\"payload\":\"");
                for _ in 0..n {
                    out.push(b'a' + (random.next() % 26) as u8);
                }
                out.extend_from_slice(b"\"}\n");
            }
            1 => {
                if out.len() > 64 {
                    let back = (window.saturating_sub(random.below(64)))
                        .min(out.len())
                        .max(1);
                    let len = 16 + random.below(4096);
                    let from = out.len() - back;
                    for i in 0..len {
                        let b = out[from + (i % back)];
                        out.push(b);
                    }
                }
            }
            2 => {
                let n = random.below(200_000);
                for _ in 0..n {
                    out.push((random.next() >> 7) as u8);
                }
            }
            3 => {
                let n = random.below(300_000);
                let b = if random.below(2) == 0 { b'x' } else { b'\n' };
                out.extend(std::iter::repeat_n(b, n));
            }
            4 => {
                let n = random.below(1 << 20);
                for i in 0..n {
                    out.push(b'A' + (i % 23) as u8);
                }
            }
            _ => out.push(b'\n'),
        }
    }
    out
}

fn compress(
    data: &[u8],
    level: i32,
    wlog: u32,
    content_size: bool,
    checksum: bool,
    ldm: bool,
) -> Vec<u8> {
    let mut c = zstd::bulk::Compressor::new(level).expect("fuzz input setup");
    c.set_parameter(CParameter::WindowLog(wlog))
        .expect("fuzz input setup");
    c.set_parameter(CParameter::ContentSizeFlag(content_size))
        .expect("fuzz input setup");
    c.set_parameter(CParameter::ChecksumFlag(checksum))
        .expect("fuzz input setup");
    c.set_parameter(CParameter::EnableLongDistanceMatching(ldm))
        .expect("fuzz input setup");
    c.compress(data).expect("fuzz input setup")
}

fn compress_streaming(data: &[u8], level: i32, wlog: u32, checksum: bool) -> Vec<u8> {
    let mut e = zstd::stream::write::Encoder::new(Vec::new(), level).expect("fuzz input setup");
    e.set_parameter(CParameter::WindowLog(wlog))
        .expect("fuzz input setup");
    e.set_parameter(CParameter::ChecksumFlag(checksum))
        .expect("fuzz input setup");
    for chunk in data.chunks(77_777) {
        e.write_all(chunk).expect("fuzz input setup");
        e.flush().expect("fuzz input setup");
    }
    e.finish().expect("fuzz input setup")
}

fn skippable(payload: &[u8]) -> Vec<u8> {
    let mut f = 0x184D_2A5F_u32.to_le_bytes().to_vec();
    f.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    f.extend_from_slice(payload);
    f
}

fn lie_window(stream: &mut [u8], random: &mut Random) {
    if stream.len() < 6 || stream[..4] != 0xFD2F_B528_u32.to_le_bytes() {
        return;
    }
    let fhd = stream[4];
    if fhd & 0x20 != 0 {
        return;
    }
    stream[5] = ((random.below(8) as u8) << 3) | (random.below(8) as u8);
}

fn mutate(stream: &[u8], random: &mut Random) -> Vec<u8> {
    let mut s = stream.to_vec();
    match random.below(6) {
        0 => s.truncate(random.below(s.len() + 1)),
        1 => {
            for _ in 0..1 + random.below(8) {
                if !s.is_empty() {
                    let i = random.below(s.len());
                    s[i] ^= 1 << random.below(8);
                }
            }
        }
        2 => {
            for _ in 0..1 + random.below(3) {
                if s.len() > 4 {
                    let i = 4 + random.below(s.len().min(24) - 4);
                    s[i] = random.next() as u8;
                }
            }
        }
        3 => lie_window(&mut s, random),
        4 => {
            if !s.is_empty() {
                let a = random.below(s.len());
                let b = a + random.below(s.len() - a);
                let dup = s[a..b].to_vec();
                let at = random.below(s.len());
                s.splice(at..at, dup);
            }
        }
        _ => {
            if !s.is_empty() {
                let i = random.below(s.len());
                s[i] ^= 0xFF;
            }
            s.truncate(random.below(s.len() + 1));
        }
    }
    s
}

fn write(dir: &Path, bytes: &[u8]) -> PathBuf {
    let p = dir.join("f.zst");
    std::fs::write(&p, bytes).expect("fuzz input setup");
    p
}

fn common_prefix_ok(a: &Outcome, b: &Outcome) -> bool {
    let n = a.lines.len().min(b.lines.len());
    a.lines[..n.saturating_sub(1)] == b.lines[..n.saturating_sub(1)]
}

#[test]
fn random_streams_and_mutations_decode_like_the_streaming_decoder() {
    let iterations: usize = setting("FUZZ_ITERS").unwrap_or(6);
    let seed: u64 = setting("FUZZ_SEED").unwrap_or(0x1234_5678_9abc_def1);
    let fixture = Fixture::new();
    let dir = fixture.path("");
    let mut random = Random(seed);
    let (mut valid, mut mutated, mut disagree_kind, mut prefix_mismatch, mut lie_mismatch) =
        (0, 0, 0, 0, 0);
    for iter in 0..iterations {
        let wlog = [10, 11, 12, 14, 15, 16, 17, 18, 20, 22, 24][random.below(11)];
        let window = 1usize << wlog;
        let size = random.below((40 * window).min(12 << 20)) + random.below(2 << 20);
        let d = data(&mut random, size, window);
        let level = [1, 3, 9, 19][random.below(4)];
        let mut stream = if random.below(3) == 0 {
            compress_streaming(&d, level, wlog, random.below(2) == 0)
        } else {
            compress(
                &d,
                level,
                wlog,
                random.below(2) == 0,
                random.below(2) == 0,
                random.below(3) == 0,
            )
        };
        let mut expected_data = d.clone();
        if random.below(3) == 0 {
            stream.extend(skippable(&d[..random.below(d.len().min(300_000))]));
            let n2 = random.below(1 << 20);
            let d2 = data(&mut random, n2, 1 << 17);
            stream.extend(compress(&d2, 3, 17, random.below(2) == 0, true, false));
            expected_data.extend_from_slice(&d2);
        }
        let capacity = [1, 61, 8192, 131072][random.below(4)];
        let path = write(&dir, &stream);
        let got = direct(&path, capacity);
        let want = reference(&path);
        assert!(
            want.error.is_none(),
            "reference failed on valid iter {iter}"
        );
        assert_eq!(
            got.lines.concat(),
            expected_data,
            "valid iter {iter} wlog {wlog} cap {capacity}"
        );
        assert_eq!(got, want, "valid iter {iter}");
        valid += 1;
        for m in 0..12 {
            let bad = mutate(&stream, &mut random);
            let lie = bad.len() >= 6 && bad[..5] == stream[..5] && bad[5] != stream[5];
            let path = write(&dir, &bad);
            let got = direct(&path, capacity);
            let want = reference(&path);
            mutated += 1;
            if got.error.as_ref().map(|e| e.0) != want.error.as_ref().map(|e| e.0) {
                disagree_kind += 1;
                let orig_ok = got.lines.concat()
                    == expected_data[..got.lines.concat().len().min(expected_data.len())];
                eprintln!(
                    "kind differs iter {iter} m {m}: direct {:?} ref {:?} lie={lie} direct_matches_original_prefix={orig_ok} got_len={} orig_len={} wlog={wlog} newwin={:#x} cap={capacity}",
                    got.error,
                    want.error,
                    got.lines.concat().len(),
                    expected_data.len(),
                    bad.get(5).copied().unwrap_or(0)
                );
            }
            if !common_prefix_ok(&got, &want) {
                if lie {
                    lie_mismatch += 1
                } else {
                    prefix_mismatch += 1;
                    eprintln!("prefix differs iter {iter} m {m}");
                }
            }
        }
    }
    eprintln!(
        "valid={valid} mutated={mutated} kind_disagree={disagree_kind} prefix_mismatch={prefix_mismatch} lie_mismatch={lie_mismatch}"
    );
}

#[test]
fn a_decode_error_ends_the_stream() {
    let fixture = Fixture::new();
    let dir = fixture.path("");
    let trials: usize = setting("FUZZ_RETRY_TRIALS").unwrap_or(40);
    let mut random = Random(7);
    let mut failed = 0;
    for trial in 0..trials {
        let d = data(&mut random, 600_000, 1 << 17);
        let content_size = trial % 2 == 0;
        let mut s = compress(&d, 3, 17, content_size, content_size, false);
        let tail = compress(b"after\n", 3, 17, false, false, false);
        let i = 20 + random.below(s.len() - 40);
        s[i] ^= 0x55;
        s.extend_from_slice(&tail);
        let path = write(&dir, &s);
        let mut reader = ZstdLines::new(File::open(&path).unwrap(), 8192).unwrap();
        let first = loop {
            match reader.next_line() {
                Ok(Some(_)) => {}
                Ok(None) => break None,
                Err(error) => break Some(error),
            }
        };
        let Some(first) = first else {
            continue;
        };
        failed += 1;
        for _ in 0..3 {
            match reader.next_line() {
                Err(error) => assert_eq!(
                    (error.kind(), error.to_string()),
                    (first.kind(), first.to_string()),
                    "trial {trial}"
                ),
                Ok(line) => panic!("trial {trial} read {line:?} after {first}"),
            }
        }
    }
    assert!(failed > 0);
}
