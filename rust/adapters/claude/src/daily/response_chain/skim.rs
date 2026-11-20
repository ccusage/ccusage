use std::sync::LazyLock;

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
    pub(super) has_usage: bool,
    pub(super) blocks: Option<SmallVec<[Block<'a>; 4]>>,
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

struct Cursor<'a> {
    line: &'a [u8],
    at: usize,
}

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

    fn link(&mut self) -> Option<Option<&'a [u8]>> {
        let link = self.nullable_string()?;
        if link.is_some_and(|link| memchr(b'\\', link).is_some()) {
            return None;
        }
        Some(link)
    }

    fn skip_value(&mut self) -> Option<()> {
        match self.peek()? {
            b'"' => self.string().map(drop),
            b'{' | b'[' => self.skip_container(),
            _ => {
                while let Some(byte) = self.line.get(self.at) {
                    if matches!(byte, b',' | b'}' | b']') || byte.is_ascii_whitespace() {
                        break;
                    }
                    self.at += 1;
                }
                Some(())
            }
        }
    }

    fn skip_container(&mut self) -> Option<()> {
        let mut depth = 0_usize;
        loop {
            match *self.line.get(self.at)? {
                b'"' => {
                    self.string()?;
                    continue;
                }
                b'{' | b'[' => depth += 1,
                b'}' | b']' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        self.at += 1;
                        return Some(());
                    }
                }
                _ => {}
            }
            self.at += 1;
        }
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
            *slot = cursor.nullable_string()?;
            Some(Step::Read)
        })?;
        Some(block)
    }

    fn message(&mut self, skim: &mut Skim<'a>) -> Option<()> {
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
            && self.seen_kind
            && match want {
                Want::Block => self.seen_message,
                Want::Link => self.seen_message || self.kind.is_some_and(|kind| kind != b"user"),
            }
    }
}

pub(super) fn skim<'a>(
    line: &'a [u8],
    want: Want,
    keep_parent: impl Fn(Option<&[u8]>) -> bool,
) -> Option<Skim<'a>> {
    let mut cursor = Cursor { line, at: 0 };
    let mut skim = Skim::default();
    cursor.object(|key, cursor| {
        match key {
            b"type" => {
                skim.kind = cursor.nullable_string()?;
                skim.seen_kind = true;
            }
            b"uuid" => {
                skim.uuid = cursor.link()?;
                skim.seen_uuid = true;
            }
            b"parentUuid" => {
                skim.parent = cursor.link()?;
                if !keep_parent(skim.parent) {
                    return None;
                }
                skim.seen_parent = true;
            }
            b"message" => {
                cursor.message(&mut skim)?;
                skim.seen_message = true;
            }
            _ => return Some(Step::Skip),
        }
        Some(if skim.complete(want) {
            Step::Stop
        } else {
            Step::Read
        })
    })?;
    if !skim.seen_parent && !keep_parent(None) {
        return None;
    }
    Some(skim)
}

#[derive(Default)]
pub(super) struct BlockLinks<'a> {
    pub(super) uuid: Option<&'a [u8]>,
    pub(super) parent: Option<&'a [u8]>,
    pub(super) tool_uses: SmallVec<[&'a [u8]; 2]>,
}

static TOOL_USE: LazyLock<Finder<'static>> = LazyLock::new(|| Finder::new(br#""type":"tool_use""#));
const UUID_MEMBER: &[u8] = br#","uuid":""#;

impl<'a> Cursor<'a> {
    fn tool_uses(
        &mut self,
        usage_at: Option<usize>,
        tool_uses: &mut SmallVec<[&'a [u8]; 2]>,
    ) -> Option<()> {
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
                    let end = usage_at
                        .filter(|&end| end > self.at)
                        .unwrap_or(self.line.len());
                    self.content_tool_uses(end, tool_uses)?;
                }
                return Some(());
            }
            self.skip_value()?;
            if self.peek()? == b',' {
                self.at += 1;
            }
        }
    }

    // NOTE(2026-11-20, #2255): A JSON string cannot hold `"type":"tool_use"` unescaped, so once no such bytes remain before the message usage, which Claude Code writes after the content, the rest of the content holds no tool use block and is not read. A layout that puts usage first only hides tool uses, which keeps responses apart.
    fn content_tool_uses(
        &mut self,
        end: usize,
        tool_uses: &mut SmallVec<[&'a [u8]; 2]>,
    ) -> Option<()> {
        self.eat(b'[')?;
        loop {
            if TOOL_USE.find(self.line.get(self.at..end)?).is_none() {
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
    };
    let uuid = cursor.link()??;
    let rest = &line[cursor.at..];
    let closes_only_the_line =
        memchr2(b'{', b'[', rest).is_none() && memchr2_iter(b'}', b']', rest).nth(1).is_none();
    if !closes_only_the_line {
        cursor.ends_object()?;
    }
    Some(uuid)
}

pub(super) fn block_links(line: &[u8], usage_at: Option<usize>) -> Option<BlockLinks<'_>> {
    let mut cursor = Cursor { line, at: 0 };
    let mut links = BlockLinks::default();
    let (mut seen_uuid, mut seen_parent) = (false, false);
    cursor.object(|key, cursor| {
        match key {
            b"uuid" => {
                links.uuid = cursor.link()?;
                seen_uuid = true;
            }
            b"parentUuid" => {
                links.parent = cursor.link()?;
                seen_parent = true;
            }
            b"message" => {
                cursor.tool_uses(usage_at, &mut links.tool_uses)?;
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
