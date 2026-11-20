use std::{
    borrow::Cow,
    hash::{Hash, Hasher},
};

use ccusage_core::{TokenUsageRaw, fast::FxHashMap};
use rustc_hash::FxHasher;
use serde::Deserialize;
use smallvec::SmallVec;

type ToolUseIds = SmallVec<[Box<str>; 2]>;

enum Node {
    Link {
        parent: Option<Box<str>>,
        answers: ToolUseIds,
    },
    Block(usize),
}

struct Response {
    key: Option<u64>,
    message_id: Box<str>,
    prompt: (u64, u64, u64),
    tool_uses: ToolUseIds,
}

// NOTE(2026-11-20, #2255): With parallel tool use, Claude Code writes the tool results of one response between its content blocks. A chain may pass through those, but a prompt or a result for another response's tool starts a new response.
#[derive(Default)]
pub(super) struct ResponseChains {
    nodes: FxHashMap<Box<str>, Node>,
    responses: Vec<Response>,
}

#[derive(Deserialize)]
struct Line<'a> {
    #[serde(borrow, rename = "type")]
    kind: Option<Cow<'a, str>>,
    #[serde(borrow)]
    uuid: Option<Cow<'a, str>>,
    #[serde(borrow, rename = "parentUuid")]
    parent_uuid: Option<Cow<'a, str>>,
    #[serde(borrow)]
    message: Option<Message<'a>>,
}

#[derive(Deserialize)]
struct Message<'a> {
    #[serde(borrow, default)]
    content: Option<Content<'a>>,
    usage: Option<serde::de::IgnoredAny>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Content<'a> {
    Blocks(#[serde(borrow)] Vec<Block<'a>>),
    Other(serde::de::IgnoredAny),
}

#[derive(Deserialize)]
struct Block<'a> {
    #[serde(borrow, rename = "type")]
    kind: Option<Cow<'a, str>>,
    #[serde(borrow)]
    id: Option<Cow<'a, str>>,
    #[serde(borrow)]
    tool_use_id: Option<Cow<'a, str>>,
}

impl Line<'_> {
    fn blocks(&self) -> &[Block<'_>] {
        match self
            .message
            .as_ref()
            .and_then(|message| message.content.as_ref())
        {
            Some(Content::Blocks(blocks)) => blocks,
            _ => &[],
        }
    }

    fn tool_uses(&self) -> ToolUseIds {
        self.blocks()
            .iter()
            .filter(|block| block.kind.as_deref() == Some("tool_use"))
            .filter_map(|block| block.id.as_deref().map(Box::from))
            .collect()
    }

    fn tool_result_answers(&self) -> Option<ToolUseIds> {
        let blocks = self.blocks();
        if blocks.is_empty() {
            return None;
        }
        blocks
            .iter()
            .map(|block| {
                (block.kind.as_deref() == Some("tool_result"))
                    .then_some(block.tool_use_id.as_deref())
                    .flatten()
                    .map(Box::from)
            })
            .collect()
    }
}

fn prompt_usage(usage: &TokenUsageRaw) -> (u64, u64, u64) {
    (
        usage.input_tokens,
        usage.cache_creation_token_count(),
        usage.cache_read_input_tokens,
    )
}

impl ResponseChains {
    pub(super) fn is_tracking(&self) -> bool {
        !self.responses.is_empty()
    }

    pub(super) fn link(&mut self, line: &[u8]) {
        let Ok(parsed) = ccusage_adapter_common::jsonl::parse::<Line<'_>>(line) else {
            return;
        };
        let Some(uuid) = parsed.uuid.as_deref() else {
            return;
        };
        if parsed
            .message
            .as_ref()
            .is_some_and(|message| message.usage.is_some())
        {
            return;
        }
        let answers = if parsed.kind.as_deref() == Some("user") {
            match parsed.tool_result_answers() {
                Some(answers) => answers,
                None => return,
            }
        } else {
            ToolUseIds::new()
        };
        self.nodes.insert(
            Box::from(uuid),
            Node::Link {
                parent: parsed.parent_uuid.as_deref().map(Box::from),
                answers,
            },
        );
    }

    pub(super) fn response_key(
        &mut self,
        line: &[u8],
        message_id: &str,
        usage: &TokenUsageRaw,
    ) -> Option<u64> {
        let parsed = ccusage_adapter_common::jsonl::parse::<Line<'_>>(line).ok();
        let uuid = parsed.as_ref().and_then(|parsed| parsed.uuid.as_deref());
        let prompt = prompt_usage(usage);
        let joined = parsed
            .as_ref()
            .and_then(|parsed| parsed.parent_uuid.as_deref())
            .and_then(|parent| self.chained_response(parent, message_id, prompt));
        let tool_uses = parsed.as_ref().map(Line::tool_uses).unwrap_or_default();
        let index = if let Some(index) = joined {
            self.responses[index].tool_uses.extend(tool_uses);
            index
        } else {
            self.responses.push(Response {
                key: uuid.map(|uuid| {
                    let mut hasher = FxHasher::default();
                    uuid.hash(&mut hasher);
                    hasher.finish()
                }),
                message_id: Box::from(message_id),
                prompt,
                tool_uses,
            });
            self.responses.len() - 1
        };
        if let Some(uuid) = uuid {
            self.nodes.insert(Box::from(uuid), Node::Block(index));
        }
        self.responses[index].key
    }

    fn chained_response(
        &self,
        parent: &str,
        message_id: &str,
        prompt: (u64, u64, u64),
    ) -> Option<usize> {
        let mut answered = SmallVec::<[&str; 4]>::new();
        let mut current = parent;
        for _ in 0..=self.nodes.len() {
            match self.nodes.get(current)? {
                Node::Link { parent, answers } => {
                    answered.extend(answers.iter().map(AsRef::as_ref));
                    current = parent.as_deref()?;
                }
                Node::Block(index) => {
                    let response = &self.responses[*index];
                    let joins = response.message_id.as_ref() == message_id
                        && response.prompt == prompt
                        && answered.iter().all(|answer| {
                            response.tool_uses.iter().any(|id| id.as_ref() == *answer)
                        });
                    return joins.then_some(*index);
                }
            }
        }
        None
    }
}
