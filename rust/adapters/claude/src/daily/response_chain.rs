use std::{
    hash::{Hash, Hasher},
    num::NonZeroU64,
};

use ccusage_core::{TokenUsageRaw, fast::FxHashMap};
use rustc_hash::FxHasher;
use smallvec::SmallVec;

mod skim;

use super::record::parse_links;

use skim::{Skim, Want, block_links, skim};

enum Node {
    Link { parent: u64, owner: Option<u32> },
    Block(u32),
}

struct Response {
    key: NonZeroU64,
    signature: u64,
}

// NOTE(2026-11-20, #2255): With parallel tool use, Claude Code writes the tool results of one response between its content blocks. A chain may pass through those, but a prompt or a result for another response's tool starts a new response.
#[derive(Default)]
pub(super) struct ResponseChains {
    nodes: FxHashMap<u64, Node>,
    responses: Vec<Response>,
    tool_use_owners: FxHashMap<u64, u32>,
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
        let (Some(uuid), Some(parent), Some(kind)) = (parsed.uuid, parsed.parent, parsed.kind)
        else {
            return;
        };
        if parsed.has_usage {
            return;
        }
        let parent = key(parent);
        let owner = if kind == b"user" {
            let Some(answers) = parsed.tool_result_answers() else {
                return;
            };
            let Some(owner) = self.response_of(parent) else {
                return;
            };
            if !answers
                .iter()
                .all(|answer| self.tool_use_owners.get(&key(answer)) == Some(&owner))
            {
                return;
            }
            Some(owner)
        } else {
            None
        };
        self.nodes.insert(key(uuid), Node::Link { parent, owner });
    }

    pub(super) fn response_key(
        &mut self,
        line: &[u8],
        usage_at: Option<usize>,
        message_id: &str,
        usage: &TokenUsageRaw,
    ) -> Option<NonZeroU64> {
        let skimmed = block_links(line, usage_at);
        let links = skimmed
            .as_ref()
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
        for tool_use in skimmed.iter().flat_map(|skimmed| &skimmed.tool_uses) {
            self.tool_use_owners.insert(key(tool_use), index);
        }
        if let Some(uuid) = links.uuid {
            self.nodes.insert(uuid, Node::Block(index));
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
