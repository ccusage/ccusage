//! Lines of a zstd-compressed file, decoded block by block straight into the buffer they are
//! sliced from through libzstd's buffer-less API (`ZSTD_decompressContinue`). This is the only
//! crate allowed `unsafe`; `direct` holds all of it.
//!
//! Between calls libzstd keeps raw pointers to two segments: the one written since the last
//! discontinuity (its prefix) and the one before it (its external dictionary). The ring follows
//! libzstd's round-buffer rule: it holds one window, two blocks and two wildcopy overlengths plus
//! the pending line and one block of headroom, and wraps when the next block may not fit. Every
//! byte a wrap or a later block overwrites then lies more than one window behind the position
//! being decoded, and a buffer replaced by a grow stays alive until the following wrap or frame.
//! The ring moves or reallocates only between frames, before `ZSTD_decompressBegin` drops the
//! old pointers.
//!
//! Rust code reaches the ring and the history only through pointers from `Vec::as_ptr` and
//! `Vec::as_mut_ptr`, reading through short-lived shared slices and moving bytes with
//! `ptr::copy`. It never creates a `&mut` slice of them, which would invalidate the pointers
//! libzstd keeps under Rust's aliasing rules.
//!
//! A frame whose header understates its window is outside the zstd format, and libzstd bounds
//! back-references only by the history it can reach, not by the declared window. The ring
//! usually keeps more history than the streaming decoder's buffer, so for such a frame the two can
//! disagree. Whenever the streaming decoder recovers the original data, this decoder returns the
//! same lines, and it accepts some frames the streaming decoder rejects. Without a frame
//! checksum, either decoder can return wrong bytes for such a frame without an error.
//!
//! If libzstd ever breaks one of its documented guarantees, the reader replays the file with the
//! `zstd` crate's streaming decoder and continues after the lines already returned.

use std::{
    fs::File,
    io::{self, BufReader, Read, Seek, SeekFrom},
};

use ccusage_core::fast::LineReader;

mod direct;

use direct::{Direct, Fault};

type Replay = LineReader<zstd::stream::read::Decoder<'static, BufReader<File>>>;

/// Lines of a zstd-compressed file, decoded straight into the buffer they are sliced from.
pub struct ZstdLines {
    inner: Inner,
    capacity: usize,
}

enum Inner {
    Direct(Direct),
    Replay(Replay),
}

impl ZstdLines {
    pub fn new(file: File, capacity: usize) -> io::Result<Self> {
        let capacity = capacity.max(1);
        Ok(Self {
            inner: Inner::Direct(Direct::new(file, capacity)?),
            capacity,
        })
    }

    /// Includes the trailing newline unless the final line is unterminated.
    /// After a read error, [`Self::rest`] holds the bytes after the last returned line.
    pub fn next_line(&mut self) -> io::Result<Option<&[u8]>> {
        let line = match &mut self.inner {
            Inner::Direct(direct) => match direct.next_line() {
                Ok(line) => Some(line),
                Err(Fault::Io(error)) => return Err(error),
                Err(Fault::Unexpected) => None,
            },
            Inner::Replay(_) => None,
        };
        match (&mut self.inner, line) {
            (Inner::Direct(direct), Some(line)) => Ok(line.map(|range| direct.line(range))),
            (inner, _) => replay(inner, self.capacity)?.next_line(),
        }
    }

    /// Bytes received after the last line [`Self::next_line`] returned.
    pub fn rest(&self) -> &[u8] {
        match &self.inner {
            Inner::Direct(direct) => direct.rest(),
            Inner::Replay(reader) => reader.rest(),
        }
    }

    #[cfg(test)]
    fn replaying(&self) -> bool {
        matches!(self.inner, Inner::Replay(_))
    }
}

fn replay(inner: &mut Inner, capacity: usize) -> io::Result<&mut Replay> {
    if let Inner::Direct(direct) = inner {
        let delivered = direct.delivered();
        let mut file = direct.file().try_clone()?;
        file.seek(SeekFrom::Start(0))?;
        let mut decoder =
            zstd::stream::read::Decoder::with_buffer(BufReader::with_capacity(capacity, file))?;
        let skipped = io::copy(&mut (&mut decoder).take(delivered), &mut io::sink())?;
        if skipped != delivered {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "zstd replay ended before the lines already returned",
            ));
        }
        *inner = Inner::Replay(LineReader::with_buffer(vec![0; capacity], decoder));
    }
    match inner {
        Inner::Replay(reader) => Ok(reader),
        Inner::Direct(_) => Err(io::Error::other("zstd replay did not start")),
    }
}

#[cfg(test)]
mod tests;
