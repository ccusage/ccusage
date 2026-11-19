use std::{fs, ops::ControlFlow};

use ccusage_adapter_common::jsonl::read_last;
use ccusage_test_support::Fixture;

#[test]
fn visits_physical_lines_in_reverse_across_chunk_boundaries() {
    let fixture = Fixture::new();
    let path = fixture.path("transcript.jsonl");
    let long = "x".repeat(17 * 1024);
    let content = format!("first\r\n\n{long}\nlast");
    fs::write(&path, content).unwrap();

    let mut visited = Vec::new();
    let accepted = read_last(&path, |mut line| {
        visited.push((line.number().unwrap(), line.raw().to_vec()));
        ControlFlow::<()>::Continue(())
    })
    .unwrap();

    assert_eq!(accepted, None);
    assert_eq!(
        visited,
        [
            (4, b"last".to_vec()),
            (3, format!("{long}\n").into_bytes()),
            (2, b"\n".to_vec()),
            (1, b"first\r\n".to_vec()),
        ]
    );
}

#[test]
fn accepts_the_first_matching_record_and_preserves_its_newline() {
    let fixture = Fixture::new();
    let path = fixture.write_file("transcript.jsonl", "older\nlatest\r\nignored\n");
    let mut seen = 0;

    let accepted = read_last(&path, |line| {
        seen += 1;
        if line.raw() == b"latest\r\n" {
            ControlFlow::Break(line.raw().to_vec())
        } else {
            ControlFlow::Continue(())
        }
    })
    .unwrap();

    assert_eq!(seen, 2);
    assert_eq!(accepted.as_deref(), Some(b"latest\r\n".as_slice()));
}

#[test]
fn an_empty_file_has_no_lines() {
    let fixture = Fixture::new();
    let path = fixture.write_file("empty.jsonl", "");

    let accepted = read_last(&path, |_| ControlFlow::Break(())).unwrap();
    assert_eq!(accepted, None);
}
