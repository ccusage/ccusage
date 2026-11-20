use serde_json::Value;
use smallvec::SmallVec;

use super::super::response_chain::{BlockLinks, LineLinks, key};

fn hashed(value: &Value) -> Option<u64> {
    value.as_str().map(|text| key(text.as_bytes()))
}

fn parse(line: &[u8]) -> Option<Value> {
    serde_json::from_slice::<Value>(line)
        .ok()
        .filter(Value::is_object)
}

pub(in crate::daily) fn parse_block_links(line: &[u8]) -> Option<BlockLinks> {
    let line = parse(line)?;
    let tool_uses = line["message"]["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "tool_use")
        .filter_map(|block| hashed(&block["id"]))
        .collect();
    Some(BlockLinks {
        uuid: hashed(&line["uuid"]),
        parent: hashed(&line["parentUuid"]),
        tool_uses,
    })
}

pub(in crate::daily) fn parse_line_links(line: &[u8]) -> Option<LineLinks> {
    let line = parse(line)?;
    let kind = line["type"].as_str()?;
    let answers = if kind == "user" {
        let blocks = line["message"]["content"]
            .as_array()
            .filter(|blocks| !blocks.is_empty())?;
        Some(
            blocks
                .iter()
                .map(|block| {
                    (block["type"] == "tool_result")
                        .then(|| hashed(&block["tool_use_id"]))
                        .flatten()
                })
                .collect::<Option<SmallVec<_>>>()?,
        )
    } else {
        None
    };
    Some(LineLinks {
        session: hashed(&line["sessionId"]),
        uuid: hashed(&line["uuid"])?,
        parent: hashed(&line["parentUuid"])?,
        has_usage: !line["message"]["usage"].is_null(),
        answers,
    })
}
