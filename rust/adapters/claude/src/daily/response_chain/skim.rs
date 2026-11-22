use std::{cell::Cell, sync::LazyLock};

use memchr::{memchr, memchr2, memchr2_iter, memmem::Finder, memrchr_iter};
use smallvec::SmallVec;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Want {
    Link,
    Block,
}

#[derive(Default)]
pub(super) struct Skim<'a> {
    pub(super) kind: Option<&'a [u8]>,
    pub(super) uuid: Option<&'a [u8]>,
    pub(super) parent: Option<&'a [u8]>,
    pub(super) session: Option<&'a [u8]>,
    pub(super) has_usage: bool,
    pub(super) blocks: Option<SmallVec<[Block<'a>; 4]>>,
    seen_session: bool,
    seen_kind: bool,
    seen_uuid: bool,
    seen_parent: bool,
    seen_message: bool,
}

#[derive(Default)]
pub(super) struct Block<'a> {
    pub(super) kind: Option<&'a [u8]>,
    id: Option<&'a [u8]>,
    pub(super) tool_use_id: Option<&'a [u8]>,
}

enum Step {
    Skip,
    Read,
    Stop,
}

// NOTE(2026-11-20, #2255): Values the scanner skips still follow JSON grammar, so a write cut short and continued by the next record cannot pass for one object. Nesting deeper than `serde_json` accepts is left to the full parse.
struct Cursor<'a> {
    line: &'a [u8],
    at: usize,
    depth: usize,
}

const MAX_DEPTH: usize = 128;

impl<'a> Cursor<'a> {
    fn peek(&mut self) -> Option<u8> {
        while self.line.get(self.at).is_some_and(u8::is_ascii_whitespace) {
            self.at += 1;
        }
        self.line.get(self.at).copied()
    }

    fn eat(&mut self, byte: u8) -> Option<()> {
        (self.peek()? == byte).then(|| self.at += 1)
    }

    fn string(&mut self) -> Option<&'a [u8]> {
        self.eat(b'"')?;
        let start = self.at;
        loop {
            let end = self.at + memchr(b'"', self.line.get(self.at..)?)?;
            self.at = end + 1;
            let escapes = self.line[start..end]
                .iter()
                .rev()
                .take_while(|&&byte| byte == b'\\')
                .count();
            if escapes % 2 == 0 {
                return Some(&self.line[start..end]);
            }
        }
    }

    fn nullable_string(&mut self) -> Option<Option<&'a [u8]>> {
        if self.peek()? == b'"' {
            self.string().map(Some)
        } else {
            self.skip_value().map(|()| None)
        }
    }

    fn plain(&mut self) -> Option<Option<&'a [u8]>> {
        let link = self.nullable_string()?;
        if link.is_some_and(|link| memchr(b'\\', link).is_some()) {
            return None;
        }
        Some(link)
    }

    fn skip_value(&mut self) -> Option<()> {
        self.depth = self.depth.checked_sub(1)?;
        let skipped = match self.peek()? {
            b'"' => self.string().map(drop),
            b'{' => self.object(|_, _| Some(Step::Skip)),
            b'[' => self.array(Self::skip_value),
            b't' => self.literal(b"true"),
            b'f' => self.literal(b"false"),
            b'n' => self.literal(b"null"),
            b'-' | b'0'..=b'9' => {
                let digits = self.line[self.at..]
                    .iter()
                    .take_while(|byte| {
                        matches!(byte, b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
                    })
                    .count();
                self.at += digits;
                Some(())
            }
            _ => None,
        };
        self.depth += 1;
        skipped
    }

    fn literal(&mut self, literal: &[u8]) -> Option<()> {
        self.line[self.at..]
            .starts_with(literal)
            .then(|| self.at += literal.len())
    }

    fn object(
        &mut self,
        mut member: impl FnMut(&'a [u8], &mut Self) -> Option<Step>,
    ) -> Option<()> {
        self.eat(b'{')?;
        if self.peek()? == b'}' {
            self.at += 1;
            return Some(());
        }
        loop {
            let key = self.string()?;
            if memchr(b'\\', key).is_some() {
                return None;
            }
            self.eat(b':')?;
            match member(key, self)? {
                Step::Skip => self.skip_value()?,
                Step::Read => {}
                Step::Stop => return Some(()),
            }
            match self.peek()? {
                b',' => self.at += 1,
                b'}' => {
                    self.at += 1;
                    return Some(());
                }
                _ => return None,
            }
        }
    }

    fn array(&mut self, mut element: impl FnMut(&mut Self) -> Option<()>) -> Option<()> {
        self.eat(b'[')?;
        if self.peek()? == b']' {
            self.at += 1;
            return Some(());
        }
        loop {
            element(self)?;
            match self.peek()? {
                b',' => self.at += 1,
                b']' => {
                    self.at += 1;
                    return Some(());
                }
                _ => return None,
            }
        }
    }

    fn block(&mut self) -> Option<Block<'a>> {
        let mut block = Block::default();
        self.object(|key, cursor| {
            let slot = match key {
                b"type" => &mut block.kind,
                b"id" => &mut block.id,
                b"tool_use_id" => &mut block.tool_use_id,
                _ => return Some(Step::Skip),
            };
            *slot = cursor.plain()?;
            Some(Step::Read)
        })?;
        Some(block)
    }

    fn message(&mut self, skim: &mut Skim<'a>, rejected: &Cell<bool>) -> Option<()> {
        if self.peek()? != b'{' {
            return self.skip_value();
        }
        self.object(|key, cursor| match key {
            b"content" => {
                if cursor.peek()? == b'[' {
                    let mut blocks = SmallVec::new();
                    cursor.array(|cursor| {
                        blocks.push(cursor.block()?);
                        Some(())
                    })?;
                    skim.blocks = Some(blocks);
                    Some(Step::Read)
                } else if skim.kind == Some(b"user") {
                    rejected.set(true);
                    None
                } else {
                    skim.blocks = None;
                    Some(Step::Skip)
                }
            }
            b"usage" => {
                skim.has_usage = cursor.peek()? != b'n';
                Some(Step::Skip)
            }
            _ => Some(Step::Skip),
        })
    }
}

impl Skim<'_> {
    fn complete(&self, want: Want) -> bool {
        self.seen_uuid
            && self.seen_parent
            && self.seen_session
            && self.seen_kind
            && match want {
                Want::Block => self.seen_message,
                Want::Link => self.seen_message || self.kind.is_some_and(|kind| kind != b"user"),
            }
    }
}

static UNICODE_ESCAPE: LazyLock<Finder<'static>> = LazyLock::new(|| Finder::new(br"\u00"));

// NOTE(2026-11-20, #2255): RFC 8259 lets a JSON string escape an ASCII letter only as `\u00XX`, and whitespace cannot split a value. Bytes that hold neither the letters nor such an escape cannot spell that value anywhere, so the scanner may stop reading there, and each marker is searched again only once the scanner passes its last match.
struct Spelling<'f> {
    finder: &'f Finder<'static>,
    next: Option<Option<usize>>,
}

impl<'f> Spelling<'f> {
    fn new(finder: &'f Finder<'static>) -> Self {
        Self { finder, next: None }
    }

    fn next_from(&mut self, line: &[u8], at: usize) -> Option<usize> {
        match self.next {
            Some(Some(next)) if next >= at => Some(next),
            Some(None) => None,
            _ => {
                let next = line
                    .get(at..)
                    .and_then(|rest| self.finder.find(rest))
                    .map(|offset| at + offset);
                self.next = Some(next);
                next
            }
        }
    }
}

pub(super) enum Scan<T> {
    Read(T),
    Skip,
    Unsure,
}

pub(super) fn skim<'a>(
    line: &'a [u8],
    want: Want,
    keep_parent: impl Fn(Option<&[u8]>, Option<&[u8]>) -> bool,
) -> Scan<Skim<'a>> {
    let mut cursor = Cursor {
        line,
        at: 0,
        depth: MAX_DEPTH,
    };
    let mut skim = Skim::default();
    let rejected = Cell::new(false);
    let mut stopped = false;
    let read = cursor.object(|key, cursor| {
        match key {
            b"type" => {
                skim.kind = cursor.plain()?;
                skim.seen_kind = true;
            }
            b"uuid" => {
                skim.uuid = cursor.plain()?;
                skim.seen_uuid = true;
            }
            b"parentUuid" => {
                skim.parent = cursor.plain()?;
                skim.seen_parent = true;
                if skim.seen_session && !keep_parent(skim.parent, skim.session) {
                    rejected.set(true);
                    return None;
                }
            }
            b"sessionId" => {
                skim.session = cursor.plain()?;
                skim.seen_session = true;
                if skim.seen_parent && !keep_parent(skim.parent, skim.session) {
                    rejected.set(true);
                    return None;
                }
            }
            b"message" => {
                cursor.message(&mut skim, &rejected)?;
                skim.seen_message = true;
            }
            _ => return Some(Step::Skip),
        }
        Some(if skim.complete(want) {
            stopped = true;
            Step::Stop
        } else {
            Step::Read
        })
    });
    // NOTE(2026-11-20, #2255): A write cut short and continued by the next record leaves one physical line that never closes its first object, which the full parse rejects. The scanner reads the rest of the line object even after it has its links and accepts the line only if that object closes at the line's end.
    let read = read.and_then(|()| {
        if stopped {
            cursor.ends_object()
        } else {
            cursor.peek().is_none().then_some(())
        }
    });
    match read {
        Some(()) if keep_parent(skim.parent, skim.session) => Scan::Read(skim),
        Some(()) => Scan::Skip,
        None if rejected.get() => Scan::Skip,
        None => Scan::Unsure,
    }
}

#[derive(Default)]
pub(super) struct RawLinks<'a> {
    pub(super) uuid: Option<&'a [u8]>,
    pub(super) parent: Option<&'a [u8]>,
    pub(super) tool_uses: SmallVec<[&'a [u8]; 2]>,
}

static TOOL_USE: LazyLock<Finder<'static>> = LazyLock::new(|| Finder::new(b"tool_use"));
const UUID_MEMBER: &[u8] = br#","uuid":""#;

impl<'a> Cursor<'a> {
    fn tool_uses(&mut self, tool_uses: &mut SmallVec<[&'a [u8]; 2]>) -> Option<()> {
        self.eat(b'{')?;
        loop {
            if self.peek()? == b'}' {
                return Some(());
            }
            let key = self.string()?;
            if memchr(b'\\', key).is_some() {
                return None;
            }
            self.eat(b':')?;
            if key == b"content" {
                if self.peek()? == b'[' {
                    self.content_tool_uses(tool_uses)?;
                }
                return Some(());
            }
            self.skip_value()?;
            if self.peek()? == b',' {
                self.at += 1;
            }
        }
    }

    fn content_tool_uses(&mut self, tool_uses: &mut SmallVec<[&'a [u8]; 2]>) -> Option<()> {
        self.eat(b'[')?;
        let mut letters = Spelling::new(&TOOL_USE);
        let mut escape = Spelling::new(&UNICODE_ESCAPE);
        loop {
            if letters.next_from(self.line, self.at).is_none()
                && escape.next_from(self.line, self.at).is_none()
            {
                return Some(());
            }
            match self.peek()? {
                b']' => return Some(()),
                b',' => self.at += 1,
                _ => {}
            }
            if self.peek()? != b'{' {
                self.skip_value()?;
                continue;
            }
            let block = self.block()?;
            if block.kind == Some(b"tool_use")
                && let Some(id) = block.id
            {
                tool_uses.push(id);
            }
        }
    }

    fn ends_object(&mut self) -> Option<()> {
        loop {
            match self.peek()? {
                b',' => {
                    self.at += 1;
                    self.string()?;
                    self.eat(b':')?;
                    self.skip_value()?;
                }
                b'}' => {
                    self.at += 1;
                    return (self.peek().is_none()).then_some(());
                }
                _ => return None,
            }
        }
    }
}

// NOTE(2026-11-20, #2255): A `uuid` member nested in an object needs at least two closing brackets after it, and brackets inside strings only add to the count, so a candidate followed by no opening bracket and a single closer is a member of the line object. Any other tail is parsed member by member.
fn top_level_uuid(line: &[u8]) -> Option<&[u8]> {
    let at = memrchr_iter(b',', line).find(|&at| line[at..].starts_with(UUID_MEMBER))?;
    let mut cursor = Cursor {
        line,
        at: at + UUID_MEMBER.len() - 1,
        depth: MAX_DEPTH,
    };
    let uuid = cursor.plain()??;
    let rest = &line[cursor.at..];
    let closes_only_the_line =
        memchr2(b'{', b'[', rest).is_none() && memchr2_iter(b'}', b']', rest).nth(1).is_none();
    if !closes_only_the_line {
        cursor.ends_object()?;
    }
    Some(uuid)
}

pub(super) fn block_links(line: &[u8]) -> Option<RawLinks<'_>> {
    let mut cursor = Cursor {
        line,
        at: 0,
        depth: MAX_DEPTH,
    };
    let mut links = RawLinks::default();
    let (mut seen_uuid, mut seen_parent) = (false, false);
    cursor.object(|key, cursor| {
        match key {
            b"uuid" => {
                links.uuid = cursor.plain()?;
                seen_uuid = true;
            }
            b"parentUuid" => {
                links.parent = cursor.plain()?;
                seen_parent = true;
            }
            b"message" => {
                cursor.tool_uses(&mut links.tool_uses)?;
                return Some(Step::Stop);
            }
            _ => return Some(Step::Skip),
        }
        Some(Step::Read)
    })?;
    if !seen_parent {
        return None;
    }
    if !seen_uuid {
        links.uuid = top_level_uuid(line);
        links.uuid?;
    }
    Some(links)
}

#[cfg(test)]
mod tests;
