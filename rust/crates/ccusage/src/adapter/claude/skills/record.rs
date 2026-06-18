use serde::Deserialize;

use crate::TokenUsageRaw;
use super::super::super::jsonl;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RecordKind {
    Assistant,
    User,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Block {
    SkillUse { id: String, name: String },
    AgentUse { id: String, subagent_type: String },
    ToolResult { tool_use_id: String, is_error: bool },
    Text { text: String },
    Other,
}

#[derive(Debug, Clone)]
pub(crate) struct Record {
    pub(crate) kind: RecordKind,
    pub(crate) timestamp: Option<String>,
    pub(crate) message_id: Option<String>,
    pub(crate) request_id: Option<String>,
    pub(crate) is_sidechain: bool,
    pub(crate) is_meta: bool,
    pub(crate) compact: bool,
    pub(crate) usage: Option<TokenUsageRaw>,
    pub(crate) model: Option<String>,
    pub(crate) blocks: Vec<Block>,
}

#[derive(Deserialize)]
struct RawRecord {
    #[serde(rename = "type")]
    rtype: Option<String>,
    subtype: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "isSidechain", default)]
    is_sidechain: bool,
    #[serde(rename = "isMeta", default)]
    is_meta: bool,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    message: Option<RawMessage>,
}

#[derive(Deserialize)]
struct RawMessage {
    id: Option<String>,
    model: Option<String>,
    usage: Option<TokenUsageRaw>,
    #[serde(default, deserialize_with = "jsonl::lenient_vec")]
    content: Vec<RawBlock>,
}

#[derive(Deserialize)]
struct RawBlock {
    #[serde(rename = "type")]
    btype: Option<String>,
    id: Option<String>,
    name: Option<String>,
    text: Option<String>,
    #[serde(rename = "tool_use_id")]
    tool_use_id: Option<String>,
    #[serde(rename = "is_error", default)]
    is_error: bool,
    #[serde(default, deserialize_with = "jsonl::lenient_object")]
    input: Option<RawInput>,
}

#[derive(Deserialize)]
struct RawInput {
    skill: Option<String>,
    subagent_type: Option<String>,
}

fn map_block(b: RawBlock) -> Block {
    match b.btype.as_deref() {
        Some("tool_use") if b.name.as_deref() == Some("Skill") => Block::SkillUse {
            id: b.id.unwrap_or_default(),
            name: b.input.and_then(|i| i.skill).unwrap_or_else(|| "<unknown>".into()),
        },
        Some("tool_use") if b.name.as_deref() == Some("Agent") || b.name.as_deref() == Some("Task") => Block::AgentUse {
            id: b.id.unwrap_or_default(),
            subagent_type: b.input.and_then(|i| i.subagent_type).unwrap_or_else(|| "<unknown>".into()),
        },
        Some("tool_result") => Block::ToolResult {
            tool_use_id: b.tool_use_id.unwrap_or_default(),
            is_error: b.is_error,
        },
        Some("text") => Block::Text { text: b.text.unwrap_or_default() },
        _ => Block::Other,
    }
}

pub(crate) fn parse_records(content: &[u8]) -> Vec<Record> {
    jsonl::records::<RawRecord>(content, None)
        .map(|r| {
            let compact = r.subtype.as_deref() == Some("compact_boundary");
            let kind = match r.rtype.as_deref() {
                Some("assistant") => RecordKind::Assistant,
                Some("user") => RecordKind::User,
                _ => RecordKind::Other,
            };
            let msg = r.message;
            Record {
                kind,
                timestamp: r.timestamp,
                message_id: msg.as_ref().and_then(|m| m.id.clone()),
                request_id: r.request_id,
                is_sidechain: r.is_sidechain,
                is_meta: r.is_meta,
                compact,
                usage: msg.as_ref().and_then(|m| m.usage),
                model: msg.as_ref().and_then(|m| m.model.clone()),
                blocks: msg.map(|m| m.content.into_iter().map(map_block).collect()).unwrap_or_default(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_skill_use_stub_body_and_usage() {
        let content = concat!(
            r#"{"type":"assistant","timestamp":"2026-06-18T10:00:00.000Z","requestId":"r1","message":{"id":"m1","model":"claude-opus-4-8","usage":{"input_tokens":5,"output_tokens":2,"cache_creation_input_tokens":10,"cache_read_input_tokens":100},"content":[{"type":"tool_use","id":"t1","name":"Skill","input":{"skill":"superpowers:brainstorming"}}]}}"#, "\n",
            r#"{"type":"user","timestamp":"2026-06-18T10:00:01.000Z","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":false}]}}"#, "\n",
            r#"{"type":"user","isMeta":true,"timestamp":"2026-06-18T10:00:02.000Z","message":{"content":[{"type":"text","text":"BODY: use writing-plans skill"}]}}"#, "\n",
            r#"{"subtype":"compact_boundary","timestamp":"2026-06-18T10:00:03.000Z"}"#, "\n",
        ).as_bytes();
        let recs = parse_records(content);
        assert_eq!(recs.len(), 4);
        assert!(matches!(recs[0].kind, RecordKind::Assistant));
        assert_eq!(recs[0].request_id.as_deref(), Some("r1"));
        assert_eq!(recs[0].message_id.as_deref(), Some("m1"));
        assert_eq!(recs[0].usage.unwrap().cache_read_input_tokens, 100);
        assert!(matches!(&recs[0].blocks[0], Block::SkillUse { id, name } if id=="t1" && name=="superpowers:brainstorming"));
        assert!(matches!(&recs[1].blocks[0], Block::ToolResult { tool_use_id, is_error:false } if tool_use_id=="t1"));
        assert!(recs[2].is_meta);
        assert!(matches!(&recs[2].blocks[0], Block::Text { text, .. } if text.contains("writing-plans")));
        assert!(recs[3].compact);
    }

    #[test]
    fn parses_agent_use_and_sidechain_flag() {
        let content = concat!(
            r#"{"type":"assistant","isSidechain":true,"requestId":"r2","message":{"id":"m2","usage":{"input_tokens":1,"output_tokens":1},"content":[{"type":"tool_use","id":"a1","name":"Agent","input":{"subagent_type":"Explore"}}]}}"#, "\n",
        ).as_bytes();
        let recs = parse_records(content);
        assert!(recs[0].is_sidechain);
        assert!(matches!(&recs[0].blocks[0], Block::AgentUse { id, subagent_type } if id=="a1" && subagent_type=="Explore"));
    }

    #[test]
    fn skips_unparseable_and_blank_lines() {
        let content = b"\n   \nnot json\n{\"type\":\"attachment\"}\n";
        let recs = parse_records(content);
        assert_eq!(recs.len(), 1);
        assert!(matches!(recs[0].kind, RecordKind::Other));
    }
}
