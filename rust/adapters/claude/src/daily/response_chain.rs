use std::{
    borrow::Cow,
    fmt,
    hash::{Hash, Hasher},
    num::NonZeroU64,
};

use ccusage_core::{TokenUsageRaw, fast::FxHashMap};
use rustc_hash::FxHasher;
use serde::{
    Deserialize, Deserializer,
    de::{IgnoredAny, MapAccess, SeqAccess, Visitor},
};
use smallvec::SmallVec;

type ToolUseIds = SmallVec<[u64; 2]>;

enum Node {
    Link {
        parent: Option<u64>,
        answers: ToolUseIds,
    },
    Block(usize),
}

struct Response {
    key: NonZeroU64,
    message_id: u64,
    prompt: (u64, u64, u64),
    tool_uses: ToolUseIds,
}

// NOTE(2026-11-20, #2255): With parallel tool use, Claude Code writes the tool results of one response between its content blocks. A chain may pass through those, but a prompt or a result for another response's tool starts a new response.
#[derive(Default)]
pub(super) struct ResponseChains {
    nodes: FxHashMap<u64, Node>,
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
    #[serde(borrow, default, deserialize_with = "content_blocks")]
    content: Option<Vec<Block<'a>>>,
    usage: Option<IgnoredAny>,
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

fn content_blocks<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<Block<'de>>>, D::Error> {
    struct Blocks;

    impl<'de> Visitor<'de> for Blocks {
        type Value = Option<Vec<Block<'de>>>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("message content")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut blocks = Vec::with_capacity(seq.size_hint().unwrap_or(1));
            while let Some(block) = seq.next_element()? {
                blocks.push(block);
            }
            Ok(Some(blocks))
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
            Ok(None)
        }

        fn visit_str<E>(self, _: &str) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
            Ok(None)
        }
    }

    deserializer.deserialize_any(Blocks)
}

fn key(value: &str) -> u64 {
    let mut hasher = FxHasher::default();
    value.hash(&mut hasher);
    hasher.finish()
}

impl Line<'_> {
    fn parse(line: &[u8]) -> Option<Line<'_>> {
        ccusage_adapter_common::jsonl::parse(line).ok()
    }

    fn blocks(&self) -> &[Block<'_>] {
        self.message
            .as_ref()
            .and_then(|message| message.content.as_deref())
            .unwrap_or_default()
    }

    fn tool_uses(&self) -> ToolUseIds {
        self.blocks()
            .iter()
            .filter(|block| block.kind.as_deref() == Some("tool_use"))
            .filter_map(|block| block.id.as_deref().map(key))
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
                    .map(key)
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
        let Some(parsed) = Line::parse(line) else {
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
            key(uuid),
            Node::Link {
                parent: parsed.parent_uuid.as_deref().map(key),
                answers,
            },
        );
    }

    pub(super) fn response_key(
        &mut self,
        line: &[u8],
        message_id: &str,
        usage: &TokenUsageRaw,
    ) -> Option<NonZeroU64> {
        let parsed = Line::parse(line);
        let uuid = parsed
            .as_ref()
            .and_then(|parsed| parsed.uuid.as_deref())
            .map(key);
        let message_id = key(message_id);
        let prompt = prompt_usage(usage);
        let joined = parsed
            .as_ref()
            .and_then(|parsed| parsed.parent_uuid.as_deref())
            .and_then(|parent| self.chained_response(key(parent), message_id, prompt));
        let tool_uses = parsed.as_ref().map(Line::tool_uses).unwrap_or_default();
        let index = if let Some(index) = joined {
            self.responses[index].tool_uses.extend(tool_uses);
            index
        } else {
            let uuid = uuid?;
            self.responses.push(Response {
                key: NonZeroU64::new(uuid).unwrap_or(NonZeroU64::MIN),
                message_id,
                prompt,
                tool_uses,
            });
            self.responses.len() - 1
        };
        if let Some(uuid) = uuid {
            self.nodes.insert(uuid, Node::Block(index));
        }
        Some(self.responses[index].key)
    }

    fn chained_response(
        &self,
        parent: u64,
        message_id: u64,
        prompt: (u64, u64, u64),
    ) -> Option<usize> {
        let mut answered = ToolUseIds::new();
        let mut current = parent;
        for _ in 0..=self.nodes.len() {
            match self.nodes.get(&current)? {
                Node::Link { parent, answers } => {
                    answered.extend_from_slice(answers);
                    current = (*parent)?;
                }
                Node::Block(index) => {
                    let response = &self.responses[*index];
                    let joins = response.message_id == message_id
                        && response.prompt == prompt
                        && answered
                            .iter()
                            .all(|answer| response.tool_uses.contains(answer));
                    return joins.then_some(*index);
                }
            }
        }
        None
    }
}
