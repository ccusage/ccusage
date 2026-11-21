#![expect(
    unsafe_code,
    reason = "calls libzstd's buffer-less streaming API, which zstd-safe does not wrap"
)]

use std::{
    cell::RefCell,
    ffi::CStr,
    fmt,
    fs::File,
    io::{self, Read},
    mem::{self, ManuallyDrop},
    ops::Range,
    ptr::{self, NonNull},
    slice,
};

use memchr::memchr;
use zstd_sys::{
    ZSTD_DCtx, ZSTD_ErrorCode, ZSTD_FrameHeader, ZSTD_FrameType_e, ZSTD_createDCtx,
    ZSTD_decompressBegin, ZSTD_decompressContinue, ZSTD_freeDCtx, ZSTD_getErrorName,
    ZSTD_getFrameHeader, ZSTD_isError, ZSTD_nextSrcSizeToDecompress,
};

const BLOCK_SIZE_MAX: usize = zstd_sys::ZSTD_BLOCKSIZE_MAX as usize;
const FRAME_HEADER_SIZE_MAX: usize = zstd_sys::ZSTD_FRAMEHEADERSIZE_MAX as usize;
const SKIPPABLE_HEADER_SIZE: usize = zstd_sys::ZSTD_SKIPPABLEHEADERSIZE as usize;
const WINDOW_SIZE_MIN: u64 = 1 << 10;
const WINDOW_SIZE_MAX: u64 = (1 << zstd_sys::ZSTD_WINDOWLOG_LIMIT_DEFAULT) + 1;
const WILDCOPY_OVERLENGTH: u64 = 32;
const CONTENT_SIZE_UNKNOWN: u64 = u64::MAX;
const RECYCLED_SIZE_MAX: usize = 16 << 20;

thread_local! {
    static BUFFERS: RefCell<(Vec<u8>, Vec<u8>)> = const { RefCell::new((Vec::new(), Vec::new())) };
    static CONTEXT: RefCell<Option<Context>> = const { RefCell::new(None) };
}

struct Ring(Vec<u8>);

impl Ring {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn bytes(&self, range: Range<usize>) -> &[u8] {
        assert!(range.start <= range.end && range.end <= self.0.len());
        let start = self.0.as_ptr().wrapping_add(range.start);
        // SAFETY: the range lies within the initialized buffer, and the pointer comes from Vec::as_ptr, which creates no reference that would invalidate the pointers libzstd keeps.
        unsafe { slice::from_raw_parts(start, range.len()) }
    }

    fn move_to_front(&mut self, range: Range<usize>) {
        assert!(range.start <= range.end && range.end <= self.0.len());
        let base = self.0.as_mut_ptr();
        // SAFETY: both ranges lie within the buffer, ptr::copy allows overlap, and the base pointer comes from Vec::as_mut_ptr like every pointer libzstd keeps.
        unsafe { ptr::copy(base.wrapping_add(range.start), base, range.len()) };
    }

    fn grown(&self, size: usize, keep: Range<usize>) -> Self {
        let mut grown = vec![0; size];
        grown[..keep.len()].copy_from_slice(self.bytes(keep));
        Self(grown)
    }

    fn tail(&mut self, start: usize) -> (*mut u8, usize) {
        let capacity = self.0.len() - start;
        if capacity == 0 {
            (ptr::null_mut(), 0)
        } else {
            (self.0.as_mut_ptr().wrapping_add(start), capacity)
        }
    }
}

pub(crate) enum Fault {
    Io(io::Error),
    Unexpected,
}

impl From<io::Error> for Fault {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

struct ZstdError(usize);

impl fmt::Debug for ZstdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Error").field(&self.name()).finish()
    }
}

impl fmt::Display for ZstdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl std::error::Error for ZstdError {}

impl ZstdError {
    fn name(&self) -> &'static str {
        // SAFETY: ZSTD_getErrorName only inspects the integer.
        let name = unsafe { ZSTD_getErrorName(self.0) };
        // SAFETY: ZSTD_getErrorName returns a static NUL-terminated string for every value.
        let name = unsafe { CStr::from_ptr(name) };
        name.to_str().unwrap_or("unknown zstd error")
    }
}

fn zstd_error(code: usize) -> Fault {
    Fault::Io(io::Error::other(ZstdError(code)))
}

fn error_code(code: ZSTD_ErrorCode) -> usize {
    0usize.wrapping_sub(code as usize)
}

fn checked(code: usize) -> Result<usize, Fault> {
    // SAFETY: ZSTD_isError only inspects the integer.
    if unsafe { ZSTD_isError(code) } != 0 {
        Err(zstd_error(code))
    } else {
        Ok(code)
    }
}

fn incomplete() -> Fault {
    Fault::Io(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "incomplete frame",
    ))
}

struct Context(NonNull<ZSTD_DCtx>);

impl Context {
    fn new() -> io::Result<Self> {
        // SAFETY: ZSTD_createDCtx has no preconditions; a null result means allocation failed.
        let context = unsafe { ZSTD_createDCtx() };
        NonNull::new(context)
            .map(Self)
            .ok_or_else(|| io::Error::from(io::ErrorKind::OutOfMemory))
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: the pointer came from ZSTD_createDCtx, is never null, and is freed only here.
        unsafe { ZSTD_freeDCtx(self.0.as_ptr()) };
    }
}

#[derive(Clone, Copy)]
enum Frame {
    Between { finished: bool },
    Skippable { remaining: u64 },
    Zstd(Layout),
}

#[derive(PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<std::time::SystemTime>,
}

impl Stamp {
    fn of(file: &File) -> io::Result<Self> {
        let metadata = file.metadata()?;
        Ok(Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

#[derive(Clone, Copy)]
struct Layout {
    size: usize,
    block: usize,
    wraps: bool,
    content: u64,
    decoded: u64,
}

impl Drop for Direct {
    fn drop(&mut self) {
        // SAFETY: `drop` runs once, and nothing reads `self.context` after this.
        let context = unsafe { ManuallyDrop::take(&mut self.context) };
        let _ = CONTEXT.try_with(|recycled| recycled.replace(Some(context)));
        let input = mem::take(&mut self.input);
        let output = mem::take(&mut self.output.0);
        if input.len() <= RECYCLED_SIZE_MAX && output.len() <= RECYCLED_SIZE_MAX {
            let _ = BUFFERS.try_with(|buffers| buffers.replace((input, output)));
        }
    }
}

pub(crate) struct Direct {
    context: ManuallyDrop<Context>,
    file: File,
    stamp: Stamp,
    input: Vec<u8>,
    input_start: usize,
    input_end: usize,
    output: Ring,
    history: Ring,
    start: usize,
    scanned: usize,
    end: usize,
    held: usize,
    deferred: Option<Fault>,
    delivered: u64,
    frame: Frame,
    eof: bool,
    #[cfg(test)]
    pub(crate) fail_after: Option<usize>,
    #[cfg(test)]
    pub(crate) grows: usize,
}

impl Direct {
    pub(crate) fn new(file: File, capacity: usize) -> io::Result<Self> {
        // NOTE(2026-11-21, #2406): ZSTD_decompressBegin resets the context before every frame. Each worker therefore keeps one instead of allocating a fresh 96 KB context per file.
        let context = match CONTEXT.try_with(RefCell::take).ok().flatten() {
            Some(context) => context,
            None => Context::new()?,
        };
        let stamp = Stamp::of(&file)?;
        // NOTE(2026-11-20): Each worker reuses its ring and input buffer across files. A fresh zero-filled ring per file cost more than the copy this decoder removes.
        let (mut input, output) = BUFFERS.try_with(RefCell::take).unwrap_or_default();
        if input.len() < capacity {
            input = vec![0; capacity];
        }
        Ok(Self {
            context: ManuallyDrop::new(context),
            file,
            stamp,
            input,
            input_start: 0,
            input_end: 0,
            output: Ring(output),
            history: Ring(Vec::new()),
            start: 0,
            scanned: 0,
            end: 0,
            held: 0,
            deferred: None,
            delivered: 0,
            frame: Frame::Between { finished: false },
            eof: false,
            #[cfg(test)]
            fail_after: None,
            #[cfg(test)]
            grows: 0,
        })
    }

    pub(crate) fn file(&self) -> &File {
        &self.file
    }

    pub(crate) fn unchanged(&self) -> io::Result<bool> {
        Ok(Stamp::of(&self.file)? == self.stamp)
    }

    pub(crate) fn delivered(&self) -> u64 {
        self.delivered
    }

    pub(crate) fn line(&self, range: Range<usize>) -> &[u8] {
        self.output.bytes(range)
    }

    pub(crate) fn rest(&self) -> &[u8] {
        self.output.bytes(self.start..self.end)
    }

    pub(crate) fn next_line(&mut self) -> Result<Option<Range<usize>>, Fault> {
        let line_end = loop {
            if let Some(newline) = memchr(b'\n', self.output.bytes(self.scanned..self.end)) {
                break self.scanned + newline + 1;
            }
            self.scanned = self.end;
            if self.eof {
                if self.start == self.end {
                    return Ok(None);
                }
                break self.end;
            }
            if let Some(fault) = self.deferred.take() {
                return Err(fault);
            }
            if let Err(fault) = self.decode() {
                // NOTE(2026-11-20): A torn frame keeps the blocks decoded before the input ended, as the streaming decoder does, so its lines come before the error. Bytes held from a damaged frame are dropped.
                if let Fault::Io(error) = &fault
                    && error.kind() == io::ErrorKind::UnexpectedEof
                    && self.held > self.end
                {
                    self.end = self.held;
                    self.deferred = Some(fault);
                    continue;
                }
                self.held = self.end;
                return Err(fault);
            }
        };
        let line = self.start..line_end;
        self.delivered += (line_end - self.start) as u64;
        self.start = line_end;
        self.scanned = line_end;
        Ok(Some(line))
    }

    fn decode(&mut self) -> Result<(), Fault> {
        loop {
            match self.frame {
                Frame::Between { finished } => {
                    if !self.fill(1)? {
                        // NOTE(2026-11-20): The zstd crate's streaming decoder reports input without one complete frame as torn, empty input included. Matching it keeps an empty rollout classified as before.
                        return if finished {
                            self.eof = true;
                            Ok(())
                        } else {
                            Err(incomplete())
                        };
                    }
                    self.begin_frame()?;
                }
                Frame::Skippable { remaining: 0 } => {
                    self.frame = Frame::Between { finished: true };
                }
                Frame::Skippable { remaining } => {
                    if !self.fill(1)? {
                        return Err(incomplete());
                    }
                    let available = self.input_end - self.input_start;
                    let skipped =
                        usize::try_from(remaining).map_or(available, |r| r.min(available));
                    self.input_start += skipped;
                    self.frame = Frame::Skippable {
                        remaining: remaining - skipped as u64,
                    };
                }
                Frame::Zstd(mut ring) => {
                    if ring.wraps && self.held + ring.block > self.output.len() {
                        // NOTE(2026-11-20): Decoded bytes stay held until the frame ends or the ring must wrap, so a frame that fits the ring is checksummed before any of its lines is returned. The streaming decoder drops what its failing call decoded, at most one block of a large frame, so this is never less strict.
                        if self.held > self.end {
                            self.end = self.held;
                            return Ok(());
                        }
                        self.wrap(ring);
                    }
                    let written = self.continue_frame(ring)?;
                    if let Frame::Zstd(current) = &mut self.frame {
                        current.decoded += written as u64;
                        ring = *current;
                    }
                    if let Frame::Between { .. } = self.frame {
                        // NOTE(2026-11-20): libzstd 1.5.7 skips the content-size check when a frame ends in an empty block. Its one-pass decoder, which the streaming decoder uses for small frames, rejects such a frame, so this keeps the verdict.
                        if ring.content != CONTENT_SIZE_UNKNOWN && ring.decoded != ring.content {
                            return Err(zstd_error(error_code(
                                ZSTD_ErrorCode::ZSTD_error_corruption_detected,
                            )));
                        }
                    }
                    if let Frame::Between { .. } = self.frame
                        && self.held > self.end
                    {
                        self.end = self.held;
                        return Ok(());
                    }
                }
            }
        }
    }

    fn begin_frame(&mut self) -> Result<(), Fault> {
        // NOTE(2026-11-20): Between frames libzstd holds no pointer into the ring. ZSTD_decompressBegin below clears the old ones, so only here may the ring move or reallocate.
        self.output.move_to_front(self.start..self.end);
        self.end -= self.start;
        self.scanned -= self.start;
        self.start = 0;
        self.held = self.end;
        self.history = Ring(Vec::new());

        let header = self.frame_header()?;
        if header.frameType == ZSTD_FrameType_e::ZSTD_skippableFrame {
            self.input_start += SKIPPABLE_HEADER_SIZE;
            self.frame = Frame::Skippable {
                remaining: header.frameContentSize,
            };
            return Ok(());
        }

        let window = header.windowSize.max(WINDOW_SIZE_MIN);
        if window > WINDOW_SIZE_MAX {
            return Err(zstd_error(error_code(
                ZSTD_ErrorCode::ZSTD_error_frameParameter_windowTooLarge,
            )));
        }
        let block = window
            .min(BLOCK_SIZE_MAX as u64)
            .min(u64::from(header.blockSizeMax));
        let ring = window + 2 * block + 2 * WILDCOPY_OVERLENGTH;
        let needed = ring.min(header.frameContentSize);
        let (Ok(ring), Ok(needed), Ok(block)) = (
            usize::try_from(ring),
            usize::try_from(needed),
            usize::try_from(block),
        ) else {
            return Err(Fault::Unexpected);
        };
        let wraps = (needed as u64) < header.frameContentSize;
        // NOTE(2026-11-20): A wrap moves the pending line in place only when this much room lies beyond the ring. Without it the first wrap would allocate a second ring per file.
        let headroom = if wraps { block } else { 0 };
        let Some(size) = self.end.checked_add(needed + headroom) else {
            return Err(Fault::Unexpected);
        };
        if self.output.len() < size {
            #[cfg(test)]
            {
                self.grows += 1;
            }
            // NOTE(2026-11-20): A line spanning many small frames would otherwise copy its pending bytes into a new ring at every frame. Doubling keeps that copying linear in the line's length.
            let size = if self.end > 0 {
                size.max(2 * self.output.len())
            } else {
                size
            };
            self.output = self.output.grown(size, 0..self.end);
        }

        // SAFETY: the context pointer is live and owned by `self`.
        checked(unsafe { ZSTD_decompressBegin(self.context.0.as_ptr()) })?;
        self.frame = Frame::Zstd(Layout {
            size: ring,
            block,
            wraps,
            content: header.frameContentSize,
            decoded: 0,
        });
        Ok(())
    }

    fn frame_header(&mut self) -> Result<ZSTD_FrameHeader, Fault> {
        let mut header = ZSTD_FrameHeader {
            frameContentSize: 0,
            windowSize: 0,
            blockSizeMax: 0,
            frameType: ZSTD_FrameType_e::ZSTD_frame,
            headerSize: 0,
            dictID: 0,
            checksumFlag: 0,
            _reserved1: 0,
            _reserved2: 0,
        };
        loop {
            let available = &self.input[self.input_start..self.input_end];
            // SAFETY: `header` is a writable ZSTD_FrameHeader and `available` is initialized for its length.
            let wanted = checked(unsafe {
                ZSTD_getFrameHeader(&raw mut header, available.as_ptr().cast(), available.len())
            })?;
            if wanted == 0 {
                return Ok(header);
            }
            if wanted <= available.len() || wanted > FRAME_HEADER_SIZE_MAX {
                return Err(Fault::Unexpected);
            }
            if !self.fill(wanted)? {
                return Err(incomplete());
            }
        }
    }

    fn continue_frame(&mut self, ring: Layout) -> Result<usize, Fault> {
        // SAFETY: the context pointer is live and owned by `self`.
        let size = unsafe { ZSTD_nextSrcSizeToDecompress(self.context.0.as_ptr()) };
        if size == 0 {
            self.frame = Frame::Between { finished: true };
            return Ok(0);
        }
        if size > BLOCK_SIZE_MAX {
            return Err(Fault::Unexpected);
        }
        let available = if self.fill(size)? {
            size
        } else {
            // NOTE(2026-11-20): At the end of the input libzstd still takes part of a raw block, as the streaming decoder does. It rejects a short unit of any other kind, which then reads as torn input.
            match self.input_end - self.input_start {
                0 => return Err(incomplete()),
                available => available,
            }
        };
        #[cfg(test)]
        if let Some(remaining) = &mut self.fail_after {
            if *remaining == 0 {
                return Err(Fault::Unexpected);
            }
            *remaining -= 1;
        }

        let (destination, capacity) = self.output.tail(self.held);
        let source = &self.input[self.input_start..self.input_start + available];
        // SAFETY: `destination` is null or `capacity` writable ring bytes and `source` is initialized input, in separate allocations. Every pointer libzstd kept is into the ring or history, both still allocated and reached since only through their Vec::as_ptr base (crate docs).
        let written = unsafe {
            ZSTD_decompressContinue(
                self.context.0.as_ptr(),
                destination.cast(),
                capacity,
                source.as_ptr().cast(),
                source.len(),
            )
        };
        let written = match checked(written) {
            Err(_) if available < size => return Err(incomplete()),
            result => result?,
        };
        if written > capacity || written > available.max(ring.block) {
            return Err(Fault::Unexpected);
        }
        self.input_start += available;
        self.held += written;
        Ok(written)
    }

    fn wrap(&mut self, ring: Layout) {
        let pending = self.end - self.start;
        if self.output.len() >= ring.size + pending {
            self.output.move_to_front(self.start..self.end);
            self.history = Ring(Vec::new());
        } else {
            let grown = self
                .output
                .grown(ring.size + 2 * pending, self.start..self.end);
            self.history = mem::replace(&mut self.output, grown);
        }
        self.scanned -= self.start;
        self.end = pending;
        self.held = pending;
        self.start = 0;
    }

    fn fill(&mut self, wanted: usize) -> io::Result<bool> {
        if self.input_end - self.input_start >= wanted {
            return Ok(true);
        }
        self.input.copy_within(self.input_start..self.input_end, 0);
        self.input_end -= self.input_start;
        self.input_start = 0;
        if self.input.len() < wanted {
            self.input.resize(wanted, 0);
        }
        while self.input_end < wanted {
            match self.file.read(&mut self.input[self.input_end..]) {
                Ok(0) => return Ok(false),
                Ok(read) => self.input_end += read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        Ok(true)
    }
}
