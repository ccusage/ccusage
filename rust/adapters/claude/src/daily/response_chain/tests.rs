use std::{collections::HashMap, num::NonZeroU64};

use ccusage_adapter_common::json::JsonText;
use ccusage_core::{Diagnostics, cost::PriceTable};
use serde_json::Value;

use super::{key, signature};
use crate::{
    Rejection,
    daily::read::{Refusal, admit, read_daily_usage_file_with_diagnostics},
    dedupe::DedupEntry as _,
};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

enum Json {
    Raw(String),
    Text(String),
    Object(Vec<(String, Json)>),
}

struct Writer<'a> {
    rng: &'a mut Rng,
    layout: Layout,
}

#[derive(Clone, Copy)]
enum Layout {
    ClaudeCode,
    MetadataFirst,
    Shuffled,
}

fn escape_one_letter(text: &str, rng: &mut Rng) -> String {
    let letters = text
        .char_indices()
        .filter(|(_, c)| c.is_ascii_alphabetic())
        .map(|(at, _)| at)
        .collect::<Vec<_>>();
    if letters.is_empty() {
        return text.to_string();
    }
    let at = *rng.pick(&letters);
    let letter = text.as_bytes()[at];
    format!("{}\\u{:04x}{}", &text[..at], letter, &text[at + 1..])
}

impl Writer<'_> {
    fn string(&mut self, text: &str, escapable: bool) -> String {
        let encoded = serde_json::to_string(text).unwrap();
        if escapable && self.rng.chance(4) {
            format!(
                "\"{}\"",
                escape_one_letter(&encoded[1..encoded.len() - 1], self.rng)
            )
        } else {
            encoded
        }
    }

    fn write(&mut self, value: &Json) -> String {
        match value {
            Json::Raw(raw) => raw.clone(),
            Json::Text(text) => self.string(text, true),
            Json::Object(members) => {
                let mut members = members.iter().collect::<Vec<_>>();
                if matches!(self.layout, Layout::Shuffled) {
                    for index in (1..members.len()).rev() {
                        let other = self.rng.below(index as u64 + 1) as usize;
                        members.swap(index, other);
                    }
                }
                let colon = if self.rng.chance(3) { ": " } else { ":" };
                let body = members
                    .into_iter()
                    .map(|(name, value)| {
                        let name = self.string(name, true);
                        let value = self.write(value);
                        format!("{name}{colon}{value}")
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                format!("{{{body}}}")
            }
        }
    }
}

fn text(value: &str) -> Json {
    Json::Text(value.to_string())
}

fn raw(value: &str) -> Json {
    Json::Raw(value.to_string())
}

fn member(name: &str, value: Json) -> (String, Json) {
    (name.to_string(), value)
}

fn parent_json(parent: Option<&str>) -> Json {
    parent.map_or_else(|| raw("null"), text)
}

fn line(
    rng: &mut Rng,
    kind: &str,
    uuid: &str,
    parent: Option<&str>,
    timestamp: &str,
    message: Option<Json>,
    extra: Vec<(String, Json)>,
) -> String {
    let layout = *rng.pick(&[
        Layout::ClaudeCode,
        Layout::ClaudeCode,
        Layout::MetadataFirst,
        Layout::Shuffled,
    ]);
    let head = vec![
        member("parentUuid", parent_json(parent)),
        member("isSidechain", raw("false")),
    ];
    let tail = vec![
        member("type", text(kind)),
        member("uuid", text(uuid)),
        member("timestamp", Json::Raw(format!("\"{timestamp}\""))),
        member(
            "sessionId",
            raw(if rng.chance(6) {
                "\"session-b\""
            } else {
                "\"session-a\""
            }),
        ),
    ];
    let mut members = head;
    match (layout, message) {
        (Layout::MetadataFirst, Some(message)) => {
            members.extend(tail);
            members.push(member("message", message));
        }
        (_, Some(message)) => {
            members.push(member("message", message));
            members.extend(tail);
        }
        (_, None) => members.extend(tail),
    }
    members.extend(extra);
    let mut writer = Writer { rng, layout };
    writer.write(&Json::Object(members))
}

fn tool_input(rng: &mut Rng, seen: &[String]) -> Json {
    let other = seen.last().cloned().unwrap_or_else(|| "nobody".to_string());
    match rng.below(6) {
        5 => Json::Object(vec![member(
            "usage",
            Json::Object(vec![member("window", text("7d"))]),
        )]),
        0 => Json::Object(vec![
            member("path", text("a")),
            member("uuid", text(&other)),
        ]),
        1 => Json::Object(vec![member("id", text("toolu_foreign"))]),
        2 => Json::Object(vec![
            member("type", text("tool_use")),
            member("id", text("toolu_inner")),
        ]),
        3 => Json::Object(vec![member(
            "list",
            Json::Raw(format!(r#"[{{"uuid":"{other}"}}]"#)),
        )]),
        _ => Json::Object(Vec::new()),
    }
}

struct Transcript {
    lines: Vec<String>,
    uuids: Vec<String>,
    tool_uses: Vec<String>,
    second: u64,
}

impl Transcript {
    fn timestamp(&mut self) -> String {
        self.second += 1;
        format!(
            "2026-05-22T02:{:02}:{:02}.000Z",
            self.second / 60 % 60,
            self.second % 60
        )
    }

    fn uuid(&mut self) -> String {
        let uuid = format!("u{}", self.uuids.len());
        self.uuids.push(uuid.clone());
        uuid
    }
}

fn transcript(rng: &mut Rng) -> Vec<String> {
    let mut t = Transcript {
        lines: Vec::new(),
        uuids: Vec::new(),
        tool_uses: Vec::new(),
        second: 0,
    };
    let mut parent: Option<String> = None;
    let mut stamps: Vec<String> = Vec::new();
    for response in 0..rng.below(5) + 1 {
        if rng.chance(25) {
            parent = None;
        }
        let message_id = if rng.chance(50) {
            "gateway".to_string()
        } else {
            format!("msg_{response}")
        };
        let input = *rng.pick(&[100, 100, 150]);
        let with_request = rng.chance(5);
        let replayed_at = (with_request && message_id.starts_with("msg_") && rng.chance(40)).then(|| {
            let at = t.timestamp();
            t.lines.push(format!(
                r#"{{"type":"assistant","isSidechain":true,"timestamp":"{at}","sessionId":"session-a","message":{{"id":"{message_id}","usage":{{"input_tokens":{input},"output_tokens":1}}}}}}"#
            ));
            at
        });
        for _ in 0..rng.below(3) + 1 {
            let timestamp = t.timestamp();
            stamps.push(timestamp.clone());
            let uuid = t.uuid();
            let is_tool_use = rng.chance(50);
            let tool_id = format!("toolu_{}", t.tool_uses.len());
            let content = if is_tool_use {
                t.tool_uses.push(tool_id.clone());
                let input = tool_input(rng, &t.uuids);
                let mut writer = Writer {
                    rng,
                    layout: Layout::ClaudeCode,
                };
                let input = writer.write(&input);
                format!(r#"[{{"type":"tool_use","id":"{tool_id}","name":"Read","input":{input}}}]"#)
            } else {
                r#"[{"type":"text","text":"say \"uuid\":\"x\" {\"type\":\"tool_use\"}"}]"#
                    .to_string()
            };
            let usage = format!(
                r#"{{"input_tokens":{input},"output_tokens":{},"cache_read_input_tokens":0}}"#,
                rng.below(3) + 1
            );
            let message = Json::Object(vec![
                member("id", text(&message_id)),
                member("model", raw("\"claude-sonnet-4-20250514\"")),
                member("role", raw("\"assistant\"")),
                member("content", Json::Raw(content)),
                member("usage", Json::Raw(usage)),
            ]);
            let mut extra = Vec::new();
            if with_request {
                extra.push(member(
                    "requestId",
                    Json::Raw(format!("\"req_{response}\"")),
                ));
            }
            if rng.chance(10) {
                extra.push(member(
                    "toolUseResult",
                    Json::Raw(format!(r#"{{"a":1,"uuid":"trail_{uuid}"}}"#)),
                ));
            }
            let block = line(
                rng,
                "assistant",
                &uuid,
                parent.as_deref(),
                &timestamp,
                Some(message),
                extra,
            );
            if replayed_at.is_none() && rng.chance(15) {
                let rewrite = t.uuid();
                let message = Json::Object(vec![
                    member("id", text(&message_id)),
                    member("role", raw("\"assistant\"")),
                    member("content", raw("[]")),
                    member(
                        "usage",
                        Json::Raw(format!(r#"{{"input_tokens":{input},"output_tokens":9}}"#)),
                    ),
                ]);
                let rewritten = line(
                    rng,
                    "assistant",
                    &rewrite,
                    parent.as_deref(),
                    &timestamp,
                    Some(message),
                    Vec::new(),
                );
                t.lines.push(block);
                t.lines.push(rewritten);
                parent = Some(if rng.chance(50) { rewrite } else { uuid });
            } else {
                t.lines.push(block);
                parent = Some(uuid);
            }
            if is_tool_use && rng.chance(15) {
                t.lines.push(format!(
                    r#"{{"parentUuid":null,"isSidechain":false,"type":"assistant","uuid":"copy_{tool_id}","timestamp":"{timestamp}","sessionId":"session-c","message":{{"id":"copy_{message_id}","role":"assistant","content":[{{"type":"tool_use","id":"{tool_id}","name":"Read","input":{{}}}}],"usage":{{"input_tokens":{input},"output_tokens":1}}}}}}"#
                ));
            }
            if replayed_at.is_none() && rng.chance(12) {
                let at = rng.pick(&stamps).clone();
                t.lines.push(format!(
                    r#"{{"type":"assistant","timestamp":"{at}","sessionId":"session-a","message":{{"id":"{message_id}","usage":{{"input_tokens":{input},"output_tokens":{}}}}}}}"#,
                    rng.below(30) + 1
                ));
            }
            match rng.below(6) {
                0 | 1 if !t.tool_uses.is_empty() => {
                    let answer = if rng.chance(70) {
                        t.tool_uses.last().unwrap().clone()
                    } else {
                        rng.pick(&t.tool_uses).clone()
                    };
                    let uuid = t.uuid();
                    let timestamp = t.timestamp();
                    let message = Json::Object(vec![
                        member("role", raw("\"user\"")),
                        member(
                            "content",
                            Json::Raw(format!(
                                r#"[{{"type":"tool_result","tool_use_id":"{answer}","content":"ok"}}]"#
                            )),
                        ),
                    ]);
                    t.lines.push(line(
                        rng,
                        "user",
                        &uuid,
                        parent.as_deref(),
                        &timestamp,
                        Some(message),
                        Vec::new(),
                    ));
                    parent = Some(uuid);
                }
                2 => {
                    let uuid = t.uuid();
                    let timestamp = t.timestamp();
                    t.lines.push(line(
                        rng,
                        "attachment",
                        &uuid,
                        parent.as_deref(),
                        &timestamp,
                        None,
                        Vec::new(),
                    ));
                    parent = Some(uuid);
                }
                _ => {}
            }
        }
        if let Some(at) = replayed_at.filter(|_| rng.chance(50)) {
            let output = if rng.chance(50) { 500 } else { 2 };
            t.lines.push(format!(
                r#"{{"type":"assistant","timestamp":"{at}","sessionId":"session-a","message":{{"id":"{message_id}","usage":{{"input_tokens":{input},"output_tokens":{output}}}}}}}"#
            ));
            if rng.chance(70) {
                let again = t.timestamp();
                t.lines.push(format!(
                    r#"{{"type":"assistant","timestamp":"{again}","sessionId":"session-a","requestId":"req_{response}","message":{{"id":"{message_id}","usage":{{"input_tokens":{input},"output_tokens":3}}}}}}"#
                ));
            }
        }
        if rng.chance(40) {
            let uuid = t.uuid();
            let timestamp = t.timestamp();
            let message = Json::Object(vec![
                member("role", raw("\"user\"")),
                member("content", text("next question")),
            ]);
            t.lines.push(line(
                rng,
                "user",
                &uuid,
                parent.as_deref(),
                &timestamp,
                Some(message),
                Vec::new(),
            ));
            parent = Some(uuid);
        }
        if rng.chance(10) {
            t.lines.push(
                r#"{"type":"api-request","sessionId":"session-a","parentUuidNote":"none"}"#
                    .to_string(),
            );
        }
    }
    interrupt_writes(rng, t.lines)
}

fn cut(rng: &mut Rng, line: &str) -> String {
    let mut at = 1 + rng.below(line.len() as u64 - 1) as usize;
    while !line.is_char_boundary(at) {
        at -= 1;
    }
    line[..at].to_string()
}

fn interrupt_writes(rng: &mut Rng, lines: Vec<String>) -> Vec<String> {
    let mut written = Vec::with_capacity(lines.len());
    for (index, line) in lines.into_iter().enumerate() {
        if rng.chance(4) {
            written.push(
                cut(rng, &line)
                    + &format!(
                        r#"{{"parentUuid":null,"isSidechain":false,"sessionId":"session-a","type":"system","uuid":"z{index}","timestamp":"2026-05-22T02:00:00.000Z"}}"#
                    ),
            );
        } else {
            written.push(line);
        }
    }
    if let Some(last) = written.last_mut().filter(|_| rng.chance(10)) {
        *last = cut(rng, last);
    }
    written
}

enum Node {
    Link { parent: u64, owner: Option<u32> },
    Block(u32),
}

#[derive(Default)]
struct Reference {
    nodes: HashMap<u64, Node>,
    responses: Vec<(NonZeroU64, u64)>,
    owners: HashMap<u64, u32>,
}

fn scoped(session: u64, uuid: u64) -> u64 {
    session.rotate_left(17) ^ uuid
}

fn hashed(value: &Value) -> Option<u64> {
    value.as_str().map(|text| key(text.as_bytes()))
}

impl Reference {
    fn response_of(&self, mut current: u64) -> Option<u32> {
        for _ in 0..=self.nodes.len() {
            match self.nodes.get(&current)? {
                Node::Link { parent, .. } => current = *parent,
                Node::Block(index) => return Some(*index),
            }
        }
        None
    }

    fn joined(&self, mut current: u64, signature: u64) -> Option<u32> {
        let mut owners = Vec::new();
        for _ in 0..=self.nodes.len() {
            match self.nodes.get(&current)? {
                Node::Link { parent, owner } => {
                    owners.extend(*owner);
                    current = *parent;
                }
                Node::Block(index) => {
                    return (self.responses[*index as usize].1 == signature
                        && owners.iter().all(|owner| owner == index))
                    .then_some(*index);
                }
            }
        }
        None
    }

    fn block(&mut self, line: &Value, signature: u64, session: u64) -> Option<NonZeroU64> {
        let uuid = hashed(&line["uuid"]);
        let index = match hashed(&line["parentUuid"])
            .and_then(|parent| self.joined(scoped(session, parent), signature))
        {
            Some(index) => index,
            None => {
                self.responses
                    .push((NonZeroU64::new(uuid?).unwrap_or(NonZeroU64::MIN), signature));
                u32::try_from(self.responses.len() - 1).ok()?
            }
        };
        for block in line["message"]["content"].as_array().into_iter().flatten() {
            if block["type"] == "tool_use"
                && let Some(id) = hashed(&block["id"])
            {
                self.owners.insert(scoped(session, id), index);
            }
        }
        if let Some(uuid) = uuid {
            self.nodes.insert(scoped(session, uuid), Node::Block(index));
        }
        Some(self.responses[index as usize].0)
    }

    fn link(&mut self, line: &Value, may_carry_usage: bool) -> Option<()> {
        let session = key(line["sessionId"].as_str().unwrap_or("session-a").as_bytes());
        let uuid = scoped(session, hashed(&line["uuid"])?);
        let parent = scoped(session, hashed(&line["parentUuid"])?);
        let kind = line["type"].as_str()?;
        if (may_carry_usage && !line["message"]["usage"].is_null())
            || !self.nodes.contains_key(&parent)
        {
            return None;
        }
        let owner = if kind == "user" {
            let blocks = line["message"]["content"]
                .as_array()
                .filter(|blocks| !blocks.is_empty())?;
            let answers = blocks
                .iter()
                .map(|block| {
                    (block["type"] == "tool_result")
                        .then(|| hashed(&block["tool_use_id"]))
                        .flatten()
                })
                .collect::<Option<Vec<_>>>()?;
            let owner = self.response_of(parent)?;
            answers
                .iter()
                .all(|answer| self.owners.get(&scoped(session, *answer)) == Some(&owner))
                .then_some(())?;
            Some(owner)
        } else {
            None
        };
        self.nodes.insert(uuid, Node::Link { parent, owner });
        Some(())
    }
}

fn reference_keys(lines: &[String]) -> Vec<Option<NonZeroU64>> {
    let mut reference = Reference::default();
    let mut keys = Vec::new();
    for line in lines {
        let value = serde_json::from_str::<Value>(line).ok();
        match admit(&JsonText::new(line.as_bytes())) {
            Ok(admitted) => {
                let record = &admitted.record;
                keys.push(match (&record.request_id, &record.message.id, &value) {
                    (None, Some(message_id), Some(value)) => reference.block(
                        value,
                        signature(message_id, &record.message.usage.usage),
                        key(record
                            .session_id
                            .as_deref()
                            .unwrap_or("session-a")
                            .as_bytes()),
                    ),
                    _ => None,
                });
            }
            Err(refusal @ (Refusal::NotUsage | Refusal::Rejected(Rejection::Malformed))) => {
                if !reference.responses.is_empty()
                    && let Some(value) = value.filter(Value::is_object)
                {
                    let _ = reference.link(&value, matches!(refusal, Refusal::Rejected(_)));
                }
            }
            Err(_) => {}
        }
    }
    keys
}

// NOTE(2026-11-20, #2255): A sidechain replay matches any record of its message and session only while it survives its slot, which a union of records cannot express. The generator gives a replayed response no requestless rewrites or unlinked records other than records at the replay's own timestamp, so this rule holds for every transcript it writes.
struct Grouped {
    identity: u64,
    response: Option<NonZeroU64>,
    timestamp: i64,
    requestless: bool,
    session: u64,
    message: u64,
    sidechain: bool,
}

fn deduped_groups(entries: &[Grouped]) -> usize {
    let mut groups = (0..entries.len()).collect::<Vec<_>>();
    fn root(groups: &mut [usize], at: usize) -> usize {
        let mut at = at;
        while groups[at] != at {
            groups[at] = groups[groups[at]];
            at = groups[at];
        }
        at
    }
    for (index, entry) in entries.iter().enumerate() {
        for (other, earlier) in entries[..index].iter().enumerate() {
            let replay = entry.message == earlier.message
                && entry.session == earlier.session
                && (entry.sidechain || earlier.sidechain);
            let same_record = entry.identity == earlier.identity
                && entry.requestless == earlier.requestless
                && (!entry.requestless
                    || (entry.session == earlier.session
                        && (entry.timestamp == earlier.timestamp
                            || (entry.response.is_some() && entry.response == earlier.response))));
            let same = replay || same_record;
            if same {
                let (a, b) = (root(&mut groups, index), root(&mut groups, other));
                groups[a] = b;
            }
        }
    }
    (0..entries.len())
        .filter(|&at| root(&mut groups, at) == at)
        .count()
}

#[test]
fn the_scanner_and_a_full_parse_place_generated_blocks_in_the_same_responses() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for case in 0..4000 {
        let lines = transcript(&mut rng);
        let fixture = ccusage_test_support::Fixture::new();
        fixture.write_file("projects/project-a/session-a.jsonl", lines.join("\n"));
        let entries = read_daily_usage_file_with_diagnostics(
            &fixture.path("projects/project-a/session-a.jsonl"),
            crate::paths::fixture_projects_root(
                &fixture.path("projects/project-a/session-a.jsonl"),
            )
            .as_deref(),
            &jiff::tz::TimeZone::UTC,
            PriceTable::Display,
            &Diagnostics::new("claude"),
        );
        let fast = entries
            .iter()
            .map(|entry| entry.key.request.response())
            .collect::<Vec<_>>();
        let expected = reference_keys(&lines);
        assert_eq!(fast, expected, "case {case}:\n{}", lines.join("\n"));

        let groups = entries
            .iter()
            .map(|entry| {
                let message = key(entry.message_id().unwrap_or_default().as_bytes());
                let identity = entry
                    .request_id()
                    .map_or(message, |request| message ^ key(request.as_bytes()));
                Grouped {
                    identity,
                    response: entry.key.request.response(),
                    timestamp: entry.timestamp().as_millis(),
                    requestless: entry.request_id().is_none(),
                    session: key(entry.session_id().as_bytes()),
                    message,
                    sidechain: entry.is_sidechain(),
                }
            })
            .collect::<Vec<_>>();
        let path = fixture.path("projects/project-a/session-a.jsonl");
        let (deduped, _) = crate::dedupe::dedupe_files(vec![(path, entries)]);
        assert_eq!(
            deduped.len(),
            deduped_groups(&groups),
            "case {case}:\n{}",
            lines.join("\n")
        );
    }
}

#[test]
fn an_unterminated_tool_result_does_not_bridge_two_responses() {
    let usage = ccusage_core::TokenUsageRaw {
        input_tokens: 100,
        ..ccusage_core::TokenUsageRaw::default()
    };
    let block = |uuid: &str, parent: &str| {
        format!(
            r#"{{"parentUuid":{parent},"isSidechain":false,"type":"assistant","uuid":"{uuid}","sessionId":"s","message":{{"id":"gw","content":[{{"type":"tool_use","id":"toolu_1","name":"Read","input":{{}}}}],"usage":{{"input_tokens":100}}}}}}"#
        )
    };
    let mut chains = super::ResponseChains::new("s");
    let first = chains.response_key(block("a1", "null").as_bytes(), "s", "gw", &usage);
    chains.link(
        br#"{"parentUuid":"a1","isSidechain":false,"sessionId":"s","type":"user","uuid":"u1","message":{"role":"user","content":[{"tool_use_id":"toolu_1","type":"tool_result","content":"ok"}]}"#,
        false,
        false,
    );
    let second = chains.response_key(block("a2", r#""u1""#).as_bytes(), "s", "gw", &usage);

    assert_ne!(first, second);
}
