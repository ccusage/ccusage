use std::{
    collections::VecDeque,
    hash::{Hash, Hasher},
    num::NonZeroU64,
};

use ccusage_core::{TokenUsageRaw, fast::FxHashMap};
use memchr::memmem;
use rustc_hash::FxHasher;
use smallvec::SmallVec;

mod skim;

use super::record::parse_links;

use skim::{Skim, Want, skim};

enum Node {
    Link { parent: u64, owner: Option<u32> },
    Block(u32),
}

struct Response {
    key: NonZeroU64,
    signature: u64,
}

struct RecentBlock {
    response: u32,
    line: Vec<u8>,
}

const RECENT_BLOCKS: usize = 64;

// NOTE(2026-11-20, #2255): With parallel tool use, Claude Code writes the tool results of one response between its content blocks. A chain may pass through those, but a prompt or a result for another response's tool starts a new response.
#[derive(Default)]
pub(super) struct ResponseChains {
    nodes: FxHashMap<u64, Node>,
    responses: Vec<Response>,
    recent: VecDeque<RecentBlock>,
}

#[derive(Debug)]
pub(super) struct Links {
    pub(super) uuid: Option<u64>,
    pub(super) parent: Option<u64>,
}

pub(super) fn key(value: &[u8]) -> u64 {
    let mut hasher = FxHasher::default();
    value.hash(&mut hasher);
    hasher.finish()
}

impl<'a> Skim<'a> {
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

impl ResponseChains {
    pub(super) fn is_tracking(&self) -> bool {
        !self.responses.is_empty()
    }

    // NOTE(2026-11-20, #2255): Claude Code writes `parentUuid` first on every line of the conversation chain, and parents precede children, so a line whose parent is not yet tracked can never lead a later block back to a response. Rejecting lines at that first key keeps requestless transcripts from paying for a full parse of every line; a chain line written another way only keeps its response from merging.
    pub(super) fn link(&mut self, line: &[u8], may_carry_usage: bool) {
        if !line.starts_with(br#"{"parentUuid":""#) {
            return;
        }
        let want = if may_carry_usage {
            Want::Block
        } else {
            Want::Link
        };
        let nodes = &self.nodes;
        let Some(parsed) = skim(line, want, |parent| {
            parent.is_some_and(|parent| nodes.contains_key(&key(parent)))
        }) else {
            return;
        };
        let (Some(uuid), Some(parent)) = (parsed.uuid, parsed.parent) else {
            return;
        };
        if parsed.has_usage {
            return;
        }
        let parent = key(parent);
        let owner = if parsed.kind == Some(b"user") {
            let Some(answers) = parsed.tool_result_answers() else {
                return;
            };
            let Some(owner) = self.response_of(parent) else {
                return;
            };
            if !answers.iter().all(|answer| self.emitted(owner, answer)) {
                return;
            }
            Some(owner)
        } else {
            None
        };
        self.nodes.insert(key(uuid), Node::Link { parent, owner });
    }

    // NOTE(2026-11-20, #2255): A tool result arrives right after the blocks of the response that asked for it, so the most recent block lines are searched for the tool use id instead of decoding the tool uses of every requestless block. A response with more blocks than are kept only fails to merge.
    fn emitted(&self, response: u32, tool_use_id: &[u8]) -> bool {
        let mut needle = Vec::with_capacity(tool_use_id.len() + 8);
        needle.extend_from_slice(br#""id":""#);
        needle.extend_from_slice(tool_use_id);
        needle.push(b'"');
        let finder = memmem::Finder::new(&needle);
        self.recent
            .iter()
            .filter(|block| block.response == response)
            .any(|block| finder.find(&block.line).is_some())
    }

    pub(super) fn response_key(
        &mut self,
        line: &[u8],
        message_id: &str,
        usage: &TokenUsageRaw,
    ) -> Option<NonZeroU64> {
        let links = skim(line, Want::Block, |_| true)
            .map(|skimmed| Links {
                uuid: skimmed.uuid.map(key),
                parent: skimmed.parent.map(key),
            })
            .or_else(|| parse_links(line))?;
        let signature = signature(message_id, usage);
        let joined = links
            .parent
            .and_then(|parent| self.chained_response(parent, signature));
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
        self.remember(index, line);
        if let Some(uuid) = links.uuid {
            self.nodes.insert(uuid, Node::Block(index));
        }
        Some(self.responses[index as usize].key)
    }

    fn remember(&mut self, response: u32, line: &[u8]) {
        let mut block = if self.recent.len() == RECENT_BLOCKS {
            self.recent.pop_front()
        } else {
            None
        }
        .unwrap_or_else(|| RecentBlock {
            response,
            line: Vec::new(),
        });
        block.response = response;
        block.line.clear();
        block.line.extend_from_slice(line);
        self.recent.push_back(block);
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
