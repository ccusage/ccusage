use std::{
    hash::{Hash, Hasher},
    num::NonZeroU64,
    sync::LazyLock,
};

use ccusage_core::{TokenUsageRaw, fast::FxHashMap};
use memchr::memmem::Finder;
use rustc_hash::FxHasher;
use smallvec::SmallVec;

mod skim;

use super::record::{parse_block_links, parse_line_links};

use skim::{Scan, Skim, Want, block_links, skim};

enum Node {
    Link { parent: u64, owner: Option<u32> },
    Block(u32),
}

struct Response {
    key: NonZeroU64,
    signature: u64,
}

// NOTE(2026-11-20, #2255): With parallel tool use, Claude Code writes the tool results of one response between its content blocks. A chain may pass through those, but a prompt or a result for another response's tool starts a new response.
pub(super) struct ResponseChains {
    path_session: u64,
    nodes: FxHashMap<u64, Node>,
    responses: Vec<Response>,
    tool_use_owners: FxHashMap<u64, u32>,
}

pub(super) struct BlockLinks {
    pub(super) uuid: Option<u64>,
    pub(super) parent: Option<u64>,
    pub(super) tool_uses: SmallVec<[u64; 2]>,
}

pub(super) struct LineLinks {
    pub(super) session: Option<u64>,
    pub(super) uuid: u64,
    pub(super) parent: u64,
    pub(super) has_usage: bool,
    pub(super) answers: Option<SmallVec<[u64; 2]>>,
}

pub(super) fn key(value: &[u8]) -> u64 {
    let mut hasher = FxHasher::default();
    value.hash(&mut hasher);
    hasher.finish()
}

impl<'a> Skim<'a> {
    fn line_links(&self) -> Option<LineLinks> {
        let answers = if self.kind? == b"user" {
            Some(
                self.tool_result_answers()?
                    .iter()
                    .map(|answer| key(answer))
                    .collect(),
            )
        } else {
            None
        };
        Some(LineLinks {
            session: self.session.map(key),
            uuid: key(self.uuid?),
            parent: key(self.parent?),
            has_usage: self.has_usage,
            answers,
        })
    }

    fn tool_result_answers(&self) -> Option<SmallVec<[&'a [u8]; 2]>> {
        let blocks = self.blocks.as_ref().filter(|blocks| !blocks.is_empty())?;
        blocks
            .iter()
            .map(|block| {
                (block.kind == Some(b"tool_result"))
                    .then_some(block.tool_use_id)
                    .flatten()
            })
            .collect()
    }
}

fn signature(message_id: &str, usage: &TokenUsageRaw) -> u64 {
    let mut hasher = FxHasher::default();
    message_id.hash(&mut hasher);
    usage.input_tokens.hash(&mut hasher);
    usage.cache_creation_token_count().hash(&mut hasher);
    usage.cache_read_input_tokens.hash(&mut hasher);
    hasher.finish()
}

// NOTE(2026-11-20, #2255): RFC 8259 lets a JSON string escape an ASCII letter only as `\u00XX`, and whitespace cannot split a name. A line holding neither `parentUuid` nor such an escape cannot have that member, and a line without a backslash holds no escape at all.
fn may_spell_parent(line: &[u8]) -> bool {
    static PARENT: LazyLock<Finder<'static>> = LazyLock::new(|| Finder::new(b"parentUuid"));
    static UNICODE_ESCAPE: LazyLock<Finder<'static>> = LazyLock::new(|| Finder::new(br"\u00"));
    PARENT.find(line).is_some()
        || (memchr::memchr(b'\\', line).is_some() && UNICODE_ESCAPE.find(line).is_some())
}

// NOTE(2026-11-20, #2255): A transcript file can hold lines of more than one session, and a block must not join a response of another session. Chain nodes and tool use owners are keyed by the line's effective session as well as its `uuid` or tool use id, since a copied transcript reuses both.
fn scoped(session: u64, uuid: u64) -> u64 {
    session.rotate_left(17) ^ uuid
}

impl ResponseChains {
    pub(super) fn new(path_session: &str) -> Self {
        Self {
            path_session: key(path_session.as_bytes()),
            nodes: FxHashMap::default(),
            responses: Vec::new(),
            tool_use_owners: FxHashMap::default(),
        }
    }

    pub(super) fn is_tracking(&self) -> bool {
        !self.responses.is_empty()
    }

    // NOTE(2026-11-20, #2255): Parents precede children, so a line whose parent is not yet tracked can never lead a later block back to a response, and the scanner rejects it at `parentUuid`. A line the scanner cannot read for certain takes a full parse instead.
    #[inline(never)]
    pub(super) fn link(&mut self, line: &[u8], may_carry_usage: bool, terminated: bool) {
        if !line.starts_with(br#"{"parentUuid":""#) && !may_spell_parent(line) {
            return;
        }
        let want = if may_carry_usage {
            Want::Block
        } else {
            Want::Link
        };
        let nodes = &self.nodes;
        let path_session = self.path_session;
        // NOTE(2026-11-20, #2255): Only the last line of a read can lack its terminator, and such a line is usually a write cut short. The full parse decides it, since the scanner may stop before the cut.
        let scanned = if terminated {
            skim(line, want, |parent, session| {
                let session = session.map_or(path_session, key);
                parent.is_some_and(|parent| nodes.contains_key(&scoped(session, key(parent))))
            })
        } else {
            Scan::Unsure
        };
        let links = match scanned {
            Scan::Read(parsed) => parsed.line_links(),
            Scan::Skip => return,
            Scan::Unsure => parse_line_links(line),
        };
        let Some(links) = links.filter(|links| !(may_carry_usage && links.has_usage)) else {
            return;
        };
        let session = links.session.unwrap_or(self.path_session);
        let (uuid, parent) = (scoped(session, links.uuid), scoped(session, links.parent));
        if !self.nodes.contains_key(&parent) {
            return;
        }
        let owner = match &links.answers {
            Some(answers) => {
                let Some(owner) = self.response_of(parent) else {
                    return;
                };
                if !answers.iter().all(|answer| {
                    self.tool_use_owners.get(&scoped(session, *answer)) == Some(&owner)
                }) {
                    return;
                }
                Some(owner)
            }
            None => None,
        };
        self.nodes.insert(uuid, Node::Link { parent, owner });
    }

    #[inline(never)]
    pub(super) fn response_key(
        &mut self,
        line: &[u8],
        session: &str,
        message_id: &str,
        usage: &TokenUsageRaw,
    ) -> Option<NonZeroU64> {
        let session = key(session.as_bytes());
        let links = block_links(line)
            .map(|raw| BlockLinks {
                uuid: raw.uuid.map(key),
                parent: raw.parent.map(key),
                tool_uses: raw.tool_uses.iter().map(|id| key(id)).collect(),
            })
            .or_else(|| parse_block_links(line))?;
        let signature = signature(message_id, usage);
        let joined = links
            .parent
            .and_then(|parent| self.chained_response(scoped(session, parent), signature));
        let index = match joined {
            Some(index) => index,
            None => {
                let index = u32::try_from(self.responses.len()).ok()?;
                self.responses.push(Response {
                    key: NonZeroU64::new(links.uuid?).unwrap_or(NonZeroU64::MIN),
                    signature,
                });
                index
            }
        };
        for &tool_use in &links.tool_uses {
            self.tool_use_owners
                .insert(scoped(session, tool_use), index);
        }
        if let Some(uuid) = links.uuid {
            self.nodes.insert(scoped(session, uuid), Node::Block(index));
        }
        Some(self.responses[index as usize].key)
    }

    fn response_of(&self, mut current: u64) -> Option<u32> {
        for _ in 0..=self.nodes.len() {
            match self.nodes.get(&current)? {
                Node::Link { parent, .. } => current = *parent,
                Node::Block(index) => return Some(*index),
            }
        }
        None
    }

    fn chained_response(&self, parent: u64, signature: u64) -> Option<u32> {
        let mut owners: SmallVec<[u32; 2]> = SmallVec::new();
        let mut current = parent;
        for _ in 0..=self.nodes.len() {
            match self.nodes.get(&current)? {
                Node::Link { parent, owner } => {
                    owners.extend(*owner);
                    current = *parent;
                }
                Node::Block(index) => {
                    let joins = self.responses[*index as usize].signature == signature
                        && owners.iter().all(|owner| owner == index);
                    return joins.then_some(*index);
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests;
