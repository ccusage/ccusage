use std::io::{self, Cursor, Read};

use super::LineReader;

fn read_lines(content: &[u8], capacity: usize) -> Vec<Vec<u8>> {
    let mut reader = LineReader::with_buffer(vec![0; capacity], Cursor::new(content));
    let mut lines = Vec::new();
    while let Some(line) = reader.next_line().unwrap() {
        lines.push(line.to_vec());
    }
    lines
}

#[test]
fn line_reader_splits_lines_across_buffer_boundaries() {
    let content = b"one\n\ntwo lines longer than the buffer\nthree";
    for capacity in [1, 2, 3, 7, 64] {
        let lines = read_lines(content, capacity);
        assert_eq!(
            lines,
            [
                b"one\n".as_slice(),
                b"\n",
                b"two lines longer than the buffer\n",
                b"three"
            ],
        );
        assert_eq!(lines.concat(), content);
    }
    assert!(read_lines(b"", 4).is_empty());
    assert_eq!(read_lines(b"one\n", 4), [b"one\n".to_vec()]);
}

struct Failing;

impl Read for Failing {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("read failed"))
    }
}

#[test]
fn line_reader_keeps_the_partial_line_before_a_read_error() {
    let mut reader =
        LineReader::with_buffer(vec![0; 4], Cursor::new(b"complete\npartial").chain(Failing));
    assert_eq!(reader.next_line().unwrap(), Some(b"complete\n".as_slice()));
    assert!(reader.next_line().is_err());
    assert_eq!(reader.rest(), b"partial");
}
