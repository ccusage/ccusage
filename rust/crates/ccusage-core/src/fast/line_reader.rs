use std::io::{self, Read};

use memchr::memchr;

pub struct LineReader<R> {
    reader: R,
    buffer: Vec<u8>,
    start: usize,
    scanned: usize,
    end: usize,
    eof: bool,
}

impl<R: Read> LineReader<R> {
    pub fn with_buffer(mut buffer: Vec<u8>, reader: R) -> Self {
        if buffer.is_empty() {
            buffer.push(0);
        }
        Self {
            reader,
            buffer,
            start: 0,
            scanned: 0,
            end: 0,
            eof: false,
        }
    }

    pub fn into_buffer(self) -> Vec<u8> {
        self.buffer
    }

    /// Includes the trailing newline unless the final line is unterminated.
    /// After a read error, [`Self::rest`] holds the bytes after the last returned line.
    pub fn next_line(&mut self) -> io::Result<Option<&[u8]>> {
        let line_end = loop {
            if let Some(newline) = memchr(b'\n', &self.buffer[self.scanned..self.end]) {
                break self.scanned + newline + 1;
            }
            self.scanned = self.end;
            if self.eof {
                if self.start == self.end {
                    return Ok(None);
                }
                break self.end;
            }
            self.fill()?;
        };
        let line = &self.buffer[self.start..line_end];
        self.start = line_end;
        self.scanned = line_end;
        Ok(Some(line))
    }

    /// Bytes received after the last line [`Self::next_line`] returned.
    pub fn rest(&self) -> &[u8] {
        &self.buffer[self.start..self.end]
    }

    fn fill(&mut self) -> io::Result<()> {
        if self.start > 0 {
            self.buffer.copy_within(self.start..self.end, 0);
            self.end -= self.start;
            self.scanned -= self.start;
            self.start = 0;
        }
        if self.end == self.buffer.len() {
            self.buffer.resize(self.buffer.len() * 2, 0);
        }
        loop {
            match self.reader.read(&mut self.buffer[self.end..]) {
                Ok(0) => {
                    self.eof = true;
                    return Ok(());
                }
                Ok(read) => {
                    self.end += read;
                    return Ok(());
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }
}

#[cfg(test)]
mod tests;
