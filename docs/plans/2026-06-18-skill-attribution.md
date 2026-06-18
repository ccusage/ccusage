# Skill Attribution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Claude-only `skills` command to the ccusage fork that attributes token spend to the skills, subagents, and context that caused it (exclusive + inclusive), plus the `/usage`-style characteristic breakdowns.

**Architecture:** A self-contained module under `rust/crates/ccusage/src/adapter/claude/skills/`. A new content-parsing pass reads `message.content` (which ccusage's hot usage-path prefilters out), assembles per-thread skill stacks, and runs the attribution math. Pure-logic cores (parse → dedup → thread/stack → attribute → dimensions) have zero ccusage deps and are unit-tested in isolation; a thin `mod.rs` wires paths, pricing, and output to the existing ccusage infrastructure.

**Tech Stack:** Rust (edition matches workspace), `serde`/`serde_json`, ccusage shared helpers (`crate::jsonl::lenient_*`, `crate::TokenUsageRaw`, `crate::calculate_cost_for_usage`, `crate::adapter::claude::paths`). Tests via `cargo test`.

## Global Constraints

- New code lives under `adapter/claude/skills/`; do not modify ccusage's existing usage-path loaders.
- Reuse `crate::TokenUsageRaw` and `crate::jsonl::lenient_*` deserializers — do not re-roll JSON parsing.
- Cost via `crate::calculate_cost_for_usage` only; never hardcode prices.
- Module privacy `pub(crate)`, matching the crate.
- Inline `#[cfg(test)] mod tests` per file, ccusage convention.
- No planning/spec/ticket citations in source comments.
- The oracle `token_estimator.py` lives in the sibling `skill-token-usage-estimator` repo; cross-checks run against it but it is not a build dependency.

---

## File Structure

- `adapter/claude/skills/mod.rs` — `run_skills(args)` command entry; path → parse → dedup → threads → attribute → dimensions → render. Owns ccusage integration.
- `adapter/claude/skills/record.rs` — typed JSONL record + content-block parsing (`Record`, `Block`).
- `adapter/claude/skills/dedup.rs` — `(message_id, request_id)` dedup incl. sidechain-replay rule.
- `adapter/claude/skills/thread.rs` — thread/segment assembly, skill stack, boundary (b), nest rule (C), per-turn ownership.
- `adapter/claude/skills/attribute.rs` — exclusive partition, cache_read apportionment, inclusive roll-up.
- `adapter/claude/skills/dimensions.rs` — high-context / subagent / long-session / plugin aggregations.
- `adapter/claude/skills/report.rs` — serde report struct + table rendering.
- Modify `adapter/claude/mod.rs` — `pub(crate) mod skills;`.
- Modify `cli.rs` — `SkillsArgs` struct + `Command::Skills` variant (mirror `SessionArgs`/`Command::Session`).
- Modify `main.rs:~132` — dispatch `Some(Command::Skills(args)) => commands::run_skills(args),` (or `adapter::claude::skills::run`).

**Naming locked across tasks** (Produces/Consumes contracts): `Record`, `Block`, `RecordKind`, `dedup(Vec<Record>) -> Vec<Record>`, `Thread`, `SkillFrame`, `Ownership`, `build_threads(&[Record], &SubagentIndex) -> Vec<Thread>`, `Attribution`, `attribute(&[Thread]) -> Attribution`, `SkillCost { exclusive: Cost, inclusive: Cost }`, `Cost { input, output, cache_creation, cache_read }`, `Dimensions`, `SkillsReport`.

---

### Task 1: Record types + content parsing

**Files:**
- Create: `rust/crates/ccusage/src/adapter/claude/skills/record.rs`
- Create: `rust/crates/ccusage/src/adapter/claude/skills/mod.rs` (module decls only, this task)
- Modify: `rust/crates/ccusage/src/adapter/claude/mod.rs` (add `pub(crate) mod skills;`)

**Interfaces:**
- Produces: `Record`, `RecordKind`, `Block`, `parse_records(content: &[u8]) -> Vec<Record>`.

- [ ] **Step 1: Declare the module.** In `adapter/claude/mod.rs` add `pub(crate) mod skills;`. In `skills/mod.rs` add:

```rust
pub(crate) mod record;
pub(crate) mod dedup;
pub(crate) mod thread;
pub(crate) mod attribute;
pub(crate) mod dimensions;
pub(crate) mod report;
```

- [ ] **Step 2: Write the failing test** in `record.rs`:

```rust
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
```

- [ ] **Step 3: Run it to confirm it fails.** Run: `cargo test -p ccusage skills::record`. Expected: FAIL (`parse_records` undefined).

- [ ] **Step 4: Implement `record.rs`:**

```rust
use serde::Deserialize;

use crate::TokenUsageRaw;
use crate::jsonl;

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
```

> Note: `jsonl::records` parses every line and silently skips lines that fail to deserialize into `RawRecord` (verified `adapter/jsonl.rs:47`). The agentProgress-wrapped shape (`data.message.message.usage`) is handled in Task 7's loader by also reading the nested form; for v1 the inline `message.usage` covers the common path. If `RawRecord` rejects nested-only lines, add an untagged enum mirroring `DailyUsageLine` (`adapter/claude/daily.rs:140-161`).

- [ ] **Step 5: Run tests to confirm pass.** Run: `cargo test -p ccusage skills::record`. Expected: PASS (3 tests).

- [ ] **Step 6: Commit.**

```bash
git add rust/crates/ccusage/src/adapter/claude/skills/ rust/crates/ccusage/src/adapter/claude/mod.rs
git commit -m "feat(skills): typed JSONL content-record parser"
```

---

### Task 2: Dedup by (message_id, request_id) with sidechain-replay rule

**Files:**
- Create: `rust/crates/ccusage/src/adapter/claude/skills/dedup.rs`

**Interfaces:**
- Consumes: `Record` (Task 1).
- Produces: `dedup(records: Vec<Record>) -> Vec<Record>`.

- [ ] **Step 1: Write the failing test:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::claude::skills::record::{Record, RecordKind};

    fn rec(mid: &str, rid: &str, side: bool) -> Record {
        Record { kind: RecordKind::Assistant, timestamp: None,
            message_id: Some(mid.into()), request_id: Some(rid.into()),
            is_sidechain: side, is_meta: false, compact: false,
            usage: None, model: None, blocks: vec![] }
    }

    #[test]
    fn drops_exact_duplicate_message_request() {
        let out = dedup(vec![rec("m1","r1",false), rec("m1","r1",false), rec("m2","r1",false)]);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn keeps_parent_when_sidechain_replays_same_message_new_request() {
        // parent (non-sidechain) then sidechain replay of same message under a new request id
        let out = dedup(vec![rec("m1","r-parent",false), rec("m1","r-replay",true)]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].request_id.as_deref(), Some("r-parent"));
    }

    #[test]
    fn records_without_ids_are_never_dropped() {
        let mut a = rec("","",false); a.message_id=None; a.request_id=None;
        let mut b = rec("","",false); b.message_id=None; b.request_id=None;
        assert_eq!(dedup(vec![a,b]).len(), 2);
    }
}
```

- [ ] **Step 2: Run to confirm fail.** Run: `cargo test -p ccusage skills::dedup`. Expected: FAIL (`dedup` undefined).

- [ ] **Step 3: Implement `dedup.rs`:**

```rust
use std::collections::HashMap;

use super::record::Record;

pub(crate) fn dedup(records: Vec<Record>) -> Vec<Record> {
    let mut out: Vec<Record> = Vec::with_capacity(records.len());
    // key: (message_id, request_id) -> index in out
    let mut exact: HashMap<(String, String), usize> = HashMap::new();
    // key: message_id -> index of a non-sidechain entry already kept
    let mut by_message: HashMap<String, usize> = HashMap::new();

    for r in records {
        let (Some(mid), Some(rid)) = (r.message_id.clone(), r.request_id.clone()) else {
            out.push(r);
            continue;
        };
        if exact.contains_key(&(mid.clone(), rid.clone())) {
            continue; // exact duplicate
        }
        // sidechain replay: same message id, but this one is sidechain and a non-sidechain
        // copy is already kept -> drop the replay (keep the parent).
        if r.is_sidechain {
            if let Some(&idx) = by_message.get(&mid) {
                if !out[idx].is_sidechain {
                    continue;
                }
            }
        }
        let idx = out.len();
        exact.insert((mid.clone(), rid), idx);
        if !r.is_sidechain {
            by_message.insert(mid, idx);
        }
        out.push(r);
    }
    out
}
```

- [ ] **Step 4: Run to confirm pass.** Run: `cargo test -p ccusage skills::dedup`. Expected: PASS (3 tests).

- [ ] **Step 5: Commit.**

```bash
git add rust/crates/ccusage/src/adapter/claude/skills/dedup.rs
git commit -m "feat(skills): dedup with sidechain-replay rule"
```

---

### Task 3: Thread / segment / skill-stack with boundary (b) + nest rule (C)

**Files:**
- Create: `rust/crates/ccusage/src/adapter/claude/skills/thread.rs`

**Interfaces:**
- Consumes: `Record`, `Block` (Task 1).
- Produces:
  - `struct Turn { skill: Option<usize>, usage: TokenUsageRaw }` (skill = index into `skills`, the leaf owner; `None` = baseline)
  - `struct SkillFrame { name: String, parent: Option<usize>, spawned_subagents: Vec<String> }` (`parent` = nesting parent index; `spawned_subagents` = Agent tool_use ids issued while this was leaf)
  - `struct Thread { id: String, is_subagent: bool, skills: Vec<SkillFrame>, turns: Vec<Turn> }`
  - `fn build_thread(records: &[Record], thread_id: String, is_subagent: bool) -> Thread`

This task builds a single thread (main or one subagent file). Cross-file subagent linkage is Task 4.

- [ ] **Step 1: Write the failing tests:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::TokenUsageRaw;
    use crate::adapter::claude::skills::record::{Block, Record, RecordKind};

    fn u(inp: u64, out: u64, cc: u64, cr: u64) -> TokenUsageRaw {
        TokenUsageRaw { input_tokens: inp, output_tokens: out,
            cache_creation_input_tokens: cc, cache_read_input_tokens: cr,
            speed: None, cache_creation: None }
    }
    fn asst(usage: TokenUsageRaw, blocks: Vec<Block>) -> Record {
        Record { kind: RecordKind::Assistant, timestamp: None, message_id: None,
            request_id: None, is_sidechain: false, is_meta: false, compact: false,
            usage: Some(usage), model: None, blocks }
    }
    fn tool_result(id: &str) -> Record {
        Record { kind: RecordKind::User, timestamp: None, message_id: None, request_id: None,
            is_sidechain: false, is_meta: false, compact: false, usage: None, model: None,
            blocks: vec![Block::ToolResult { tool_use_id: id.into(), is_error: false }] }
    }
    fn body(text: &str) -> Record {
        Record { kind: RecordKind::User, timestamp: None, message_id: None, request_id: None,
            is_sidechain: false, is_meta: true, compact: false, usage: None, model: None,
            blocks: vec![Block::Text { text: text.into() }] }
    }
    fn skill(id: &str, name: &str) -> Block { Block::SkillUse { id: id.into(), name: name.into() } }
    fn boundary() -> Record {
        Record { kind: RecordKind::Other, timestamp: None, message_id: None, request_id: None,
            is_sidechain: false, is_meta: false, compact: true, usage: None, model: None, blocks: vec![] }
    }

    #[test]
    fn leaf_ownership_partitions_turns() {
        let t = build_thread(&[
            asst(u(1,1,0,0), vec![skill("t1","A")]),   // invoke A
            tool_result("t1"), body("A body"),
            asst(u(2,2,0,0), vec![]),                  // owned by A
            asst(u(3,3,0,0), vec![skill("t2","B")]),   // invoke B (A body has no ref to B -> sibling)
            tool_result("t2"), body("B body"),
            asst(u(4,4,0,0), vec![]),                  // owned by B
        ], "s".into(), false);
        assert_eq!(t.skills.len(), 2);
        // turn ownership: first asst created A (its own turn owned by A), then A, then B-create, then B
        let owners: Vec<Option<&str>> = t.turns.iter()
            .map(|tn| tn.skill.map(|i| t.skills[i].name.as_str())).collect();
        assert_eq!(owners, vec![Some("A"), Some("A"), Some("B"), Some("B")]);
        assert_eq!(t.skills[1].parent, None); // B is a sibling, not nested under A
    }

    #[test]
    fn nest_when_parent_body_references_child() {
        let t = build_thread(&[
            asst(u(1,1,0,0), vec![skill("t1","brainstorming")]),
            tool_result("t1"), body("then invoke writing-plans skill"),
            asst(u(1,1,0,0), vec![skill("t2","writing-plans")]),  // referenced by A body -> nest
            tool_result("t2"), body("wp body"),
            asst(u(1,1,0,0), vec![]),
        ], "s".into(), false);
        let wp = t.skills.iter().position(|s| s.name=="writing-plans").unwrap();
        let br = t.skills.iter().position(|s| s.name=="brainstorming").unwrap();
        assert_eq!(t.skills[wp].parent, Some(br));
    }

    #[test]
    fn boundary_pops_whole_stack() {
        let t = build_thread(&[
            asst(u(1,1,0,0), vec![skill("t1","A")]), tool_result("t1"), body("A"),
            boundary(),
            asst(u(9,9,0,0), vec![]),  // after compaction, A is gone -> baseline
        ], "s".into(), false);
        assert_eq!(t.turns.last().unwrap().skill, None);
    }
}
```

- [ ] **Step 2: Run to confirm fail.** Run: `cargo test -p ccusage skills::thread`. Expected: FAIL.

- [ ] **Step 3: Implement `thread.rs`:**

```rust
use crate::TokenUsageRaw;

use super::record::{Block, Record, RecordKind};

#[derive(Debug, Clone)]
pub(crate) struct Turn {
    pub(crate) skill: Option<usize>,
    pub(crate) usage: TokenUsageRaw,
}

#[derive(Debug, Clone)]
pub(crate) struct SkillFrame {
    pub(crate) name: String,
    pub(crate) parent: Option<usize>,
    pub(crate) body: String,
    pub(crate) spawned_subagents: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Thread {
    pub(crate) id: String,
    pub(crate) is_subagent: bool,
    pub(crate) skills: Vec<SkillFrame>,
    pub(crate) turns: Vec<Turn>,
}

/// Returns true if `parent_body` references `child` by namespaced id or bare name.
fn body_references(parent_body: &str, child: &str) -> bool {
    if parent_body.contains(child) {
        return true;
    }
    // bare-name match for "superpowers:writing-plans" referenced as "writing-plans"
    let bare = child.rsplit(':').next().unwrap_or(child);
    !bare.is_empty() && parent_body.contains(bare)
}

pub(crate) fn build_thread(records: &[Record], id: String, is_subagent: bool) -> Thread {
    let mut skills: Vec<SkillFrame> = Vec::new();
    let mut turns: Vec<Turn> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    // pending skill invocations awaiting their tool_result stub: tool_use_id -> skill index
    let mut pending: Vec<(String, usize)> = Vec::new();
    // a skill whose stub succeeded and whose body (next isMeta text) we still need
    let mut awaiting_body: Option<usize> = None;

    for r in records {
        if r.compact {
            stack.clear();
            awaiting_body = None;
            continue;
        }

        match r.kind {
            RecordKind::Assistant => {
                awaiting_body = None;
                if let Some(usage) = r.usage {
                    turns.push(Turn { skill: stack.last().copied(), usage });
                }
                for b in &r.blocks {
                    match b {
                        Block::SkillUse { id: tid, name } => {
                            let idx = push_skill(&mut skills, &mut stack, name);
                            pending.push((tid.clone(), idx));
                        }
                        Block::AgentUse { id: aid, .. } => {
                            if let Some(&leaf) = stack.last() {
                                skills[leaf].spawned_subagents.push(aid.clone());
                            }
                        }
                        _ => {}
                    }
                }
            }
            RecordKind::User | RecordKind::Other => {
                for b in &r.blocks {
                    if let Block::ToolResult { tool_use_id, is_error } = b {
                        if let Some(pos) = pending.iter().position(|(t, _)| t == tool_use_id) {
                            let (_, idx) = pending.remove(pos);
                            if *is_error {
                                // failed load: pop it back off the stack if it's the leaf
                                if stack.last() == Some(&idx) {
                                    stack.pop();
                                }
                            } else {
                                awaiting_body = Some(idx);
                            }
                        }
                    }
                }
                if let Some(idx) = awaiting_body {
                    let text: String = r.blocks.iter().filter_map(|b| match b {
                        Block::Text { text } => Some(text.as_str()), _ => None,
                    }).collect();
                    if !text.is_empty() {
                        skills[idx].body = text;
                        awaiting_body = None;
                    }
                }
            }
        }
    }
    Thread { id, is_subagent, skills, turns }
}

/// Push a new skill, applying nest rule (C): nest under the deepest stack frame whose
/// body references this skill; otherwise it is a sibling (clear the stack to root first).
fn push_skill(skills: &mut Vec<SkillFrame>, stack: &mut Vec<usize>, name: &str) -> usize {
    let parent = stack.iter().rev().copied()
        .find(|&a| body_references(&skills[a].body, name));
    match parent {
        Some(a) => {
            // pop everything above `a`
            while stack.last().copied() != Some(a) {
                stack.pop();
            }
        }
        None => stack.clear(),
    }
    let idx = skills.len();
    skills.push(SkillFrame { name: name.to_string(), parent, body: String::new(), spawned_subagents: vec![] });
    stack.push(idx);
    idx
}
```

> Note: a skill's `body` is captured *after* invocation, so a parent reference is only known once the parent's body has loaded — which always precedes a child invocation in practice (the parent must run to invoke the child). The leaf at invocation time already has its body.

- [ ] **Step 4: Run to confirm pass.** Run: `cargo test -p ccusage skills::thread`. Expected: PASS (3 tests).

- [ ] **Step 5: Commit.**

```bash
git add rust/crates/ccusage/src/adapter/claude/skills/thread.rs
git commit -m "feat(skills): thread assembly, skill stack, boundary + nest rules"
```

---

### Task 4: Subagent thread linkage

**Files:**
- Modify: `rust/crates/ccusage/src/adapter/claude/skills/thread.rs`

**Interfaces:**
- Consumes: `Thread`, `SkillFrame.spawned_subagents` (Task 3).
- Produces:
  - `struct ThreadGraph { threads: Vec<Thread>, links: Vec<SubagentLink> }`
  - `struct SubagentLink { parent_thread: usize, parent_skill: Option<usize>, child_thread: usize }`
  - `fn link_subagents(main: Thread, subagents: Vec<(String, Thread)>) -> ThreadGraph` where the `String` is the subagent file's `agent` id parsed from its path/first record.

For v1, link by **spawn order within a session**: each `Agent` tool_use id in the main thread (in `spawned_subagents`) maps to one subagent file, matched by order of appearance (subagent files sorted by first timestamp). This avoids needing an explicit id join the transcript may not provide.

- [ ] **Step 1: Write the failing test:**

```rust
#[test]
fn links_subagents_to_spawning_skill_by_order() {
    let mut main = Thread { id: "s".into(), is_subagent: false, skills: vec![
        SkillFrame { name: "sdd".into(), parent: None, body: String::new(), spawned_subagents: vec!["a1".into(), "a2".into()] },
    ], turns: vec![] };
    // leaf skill 0 spawned a1, a2
    let child1 = Thread { id: "sub1".into(), is_subagent: true, skills: vec![], turns: vec![] };
    let child2 = Thread { id: "sub2".into(), is_subagent: true, skills: vec![], turns: vec![] };
    let g = link_subagents(main.clone(), vec![("sub1".into(), child1), ("sub2".into(), child2)]);
    assert_eq!(g.threads.len(), 3);
    assert_eq!(g.links.len(), 2);
    assert!(g.links.iter().all(|l| l.parent_thread == 0 && l.parent_skill == Some(0)));
    let child_threads: Vec<usize> = g.links.iter().map(|l| l.child_thread).collect();
    assert_eq!(child_threads, vec![1, 2]);
    let _ = &mut main;
}
```

- [ ] **Step 2: Run to confirm fail.** Run: `cargo test -p ccusage skills::thread::tests::links_subagents`. Expected: FAIL.

- [ ] **Step 3: Implement** in `thread.rs`:

```rust
#[derive(Debug, Clone)]
pub(crate) struct SubagentLink {
    pub(crate) parent_thread: usize,
    pub(crate) parent_skill: Option<usize>,
    pub(crate) child_thread: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct ThreadGraph {
    pub(crate) threads: Vec<Thread>,
    pub(crate) links: Vec<SubagentLink>,
}

pub(crate) fn link_subagents(main: Thread, subagents: Vec<(String, Thread)>) -> ThreadGraph {
    // ordered list of (spawning_skill_index, agent_id) from the main thread
    let mut spawns: Vec<(usize, String)> = Vec::new();
    for (si, s) in main.skills.iter().enumerate() {
        for aid in &s.spawned_subagents {
            spawns.push((si, aid.clone()));
        }
    }
    let mut threads = vec![main];
    let mut links = Vec::new();
    for (i, (_child_id, child)) in subagents.into_iter().enumerate() {
        let child_thread = threads.len();
        threads.push(child);
        let parent_skill = spawns.get(i).map(|(si, _)| *si);
        links.push(SubagentLink { parent_thread: 0, parent_skill, child_thread });
    }
    ThreadGraph { threads, links }
}
```

> Note: order-based linkage is an approximation. If transcripts expose the spawning `tool_use_id` inside the subagent file (check the first record's parent id during Task 9 corpus run), replace the order match with an id join. Flag in the report that subagent linkage is heuristic.

- [ ] **Step 4: Run to confirm pass.** Run: `cargo test -p ccusage skills::thread::tests::links_subagents`. Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add rust/crates/ccusage/src/adapter/claude/skills/thread.rs
git commit -m "feat(skills): link subagent threads to spawning skill"
```

---

### Task 5: Attribution — exclusive partition + cache_read apportionment

**Files:**
- Create: `rust/crates/ccusage/src/adapter/claude/skills/attribute.rs`

**Interfaces:**
- Consumes: `Thread`, `Turn` (Task 3).
- Produces:
  - `struct Cost { pub input: f64, pub output: f64, pub cache_creation: f64, pub cache_read: f64 }` with `fn total(&self) -> f64`
  - `fn exclusive_per_thread(thread: &Thread) -> (Vec<Cost>, Cost)` returning per-skill exclusive cost (indexed like `thread.skills`) and the baseline cost.

Apportionment: process turns in order; `added[s] += cc` for the owning skill; `prefix += cc`; for each turn split `cr` across all skills by `added[s] / prefix`; `out`, `cc`, `in` go to the owner directly.

- [ ] **Step 1: Write the failing test** (hand-computed):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::TokenUsageRaw;
    use crate::adapter::claude::skills::thread::{SkillFrame, Thread, Turn};

    fn u(inp:u64,out:u64,cc:u64,cr:u64)->TokenUsageRaw{TokenUsageRaw{input_tokens:inp,output_tokens:out,cache_creation_input_tokens:cc,cache_read_input_tokens:cr,speed:None,cache_creation:None}}
    fn frame(n:&str)->SkillFrame{SkillFrame{name:n.into(),parent:None,body:String::new(),spawned_subagents:vec![]}}

    #[test]
    fn apportions_cache_read_by_injected_cc_share() {
        // turn0: skill A owns, cc=100, cr=0
        // turn1: skill B owns, cc=100, cr=200  -> prefix before t1 = 100 (all A) => all 200 cr to A
        // turn2: skill B owns, cc=0,   cr=400  -> prefix=200 (A=100,B=100) => 200 to A, 200 to B
        let t = Thread { id:"s".into(), is_subagent:false,
            skills: vec![frame("A"), frame("B")],
            turns: vec![
                Turn{skill:Some(0), usage:u(0,0,100,0)},
                Turn{skill:Some(1), usage:u(0,0,100,200)},
                Turn{skill:Some(1), usage:u(0,0,0,400)},
            ]};
        let (per, base) = exclusive_per_thread(&t);
        assert_eq!(per[0].cache_creation, 100.0);
        assert_eq!(per[1].cache_creation, 100.0);
        assert_eq!(per[0].cache_read, 400.0); // 200 + 200
        assert_eq!(per[1].cache_read, 200.0); // 0 + 200
        assert_eq!(base.total(), 0.0);
        // reconciliation: sum cr == billed cr (600)
        assert_eq!(per[0].cache_read + per[1].cache_read, 600.0);
    }

    #[test]
    fn baseline_owns_unattributed_turns() {
        let t = Thread { id:"s".into(), is_subagent:false, skills: vec![frame("A")],
            turns: vec![ Turn{skill:None, usage:u(5,5,0,0)}, Turn{skill:Some(0), usage:u(1,1,0,0)} ]};
        let (per, base) = exclusive_per_thread(&t);
        assert_eq!(base.input, 5.0); assert_eq!(base.output, 5.0);
        assert_eq!(per[0].input, 1.0);
    }
}
```

- [ ] **Step 2: Run to confirm fail.** Run: `cargo test -p ccusage skills::attribute`. Expected: FAIL.

- [ ] **Step 3: Implement `attribute.rs`:**

```rust
use super::thread::Thread;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Cost {
    pub(crate) input: f64,
    pub(crate) output: f64,
    pub(crate) cache_creation: f64,
    pub(crate) cache_read: f64,
}

impl Cost {
    pub(crate) fn total(&self) -> f64 {
        self.input + self.output + self.cache_creation + self.cache_read
    }
    fn add_owner(&mut self, inp: u64, out: u64, cc: u64) {
        self.input += inp as f64;
        self.output += out as f64;
        self.cache_creation += cc as f64;
    }
}

/// Exclusive cost per skill (indexed like `thread.skills`) plus baseline.
pub(crate) fn exclusive_per_thread(thread: &Thread) -> (Vec<Cost>, Cost) {
    let n = thread.skills.len();
    let mut per = vec![Cost::default(); n];
    let mut base = Cost::default();
    // cumulative cache_creation injected so far, per owner slot (None -> baseline at index n)
    let mut added = vec![0.0_f64; n + 1];
    let mut prefix = 0.0_f64;

    for turn in &thread.turns {
        let owner = turn.skill.unwrap_or(n);
        let usage = turn.usage;
        let cr = usage.cache_read_input_tokens as f64;

        // apportion this turn's cache_read across everything injected so far
        if cr > 0.0 && prefix > 0.0 {
            for slot in 0..=n {
                if added[slot] > 0.0 {
                    let share = cr * (added[slot] / prefix);
                    if slot == n { base.cache_read += share; } else { per[slot].cache_read += share; }
                }
            }
        }

        let cc = usage.cache_creation_token_count();
        if owner == n {
            base.add_owner(usage.input_tokens, usage.output_tokens, cc);
        } else {
            per[owner].add_owner(usage.input_tokens, usage.output_tokens, cc);
        }
        added[owner] += cc as f64;
        prefix += cc as f64;
    }
    (per, base)
}
```

> `cache_creation_token_count()` is the ccusage helper summing structured 5m/1h (`types.rs:42`).

- [ ] **Step 4: Run to confirm pass.** Run: `cargo test -p ccusage skills::attribute`. Expected: PASS (2 tests).

- [ ] **Step 5: Commit.**

```bash
git add rust/crates/ccusage/src/adapter/claude/skills/attribute.rs
git commit -m "feat(skills): exclusive attribution with cache_read apportionment"
```

---

### Task 6: Inclusive roll-up (nesting + subagent subtree)

**Files:**
- Modify: `rust/crates/ccusage/src/adapter/claude/skills/attribute.rs`

**Interfaces:**
- Consumes: `ThreadGraph`, `Cost`, `exclusive_per_thread` (Tasks 4, 5).
- Produces:
  - `struct SkillCost { pub thread: usize, pub skill: usize, pub name: String, pub exclusive: Cost, pub inclusive: Cost }`
  - `struct Attribution { pub skills: Vec<SkillCost>, pub baseline: Cost }`
  - `fn attribute(graph: &ThreadGraph) -> Attribution`

Inclusive(S) = exclusive(S) + Σ inclusive(children). Children = skills whose `parent == S` in the same thread, **plus** the root skills of subagent threads linked to S (via `SubagentLink.parent_skill`). A subagent thread with no owning skill contributes its baseline+skills to the linking skill's inclusive.

- [ ] **Step 1: Write the failing test:**

```rust
#[test]
fn inclusive_rolls_up_nested_and_subagent_costs() {
    use crate::adapter::claude::skills::thread::{SkillFrame, SubagentLink, Thread, ThreadGraph, Turn};
    use crate::TokenUsageRaw;
    let u = |o:u64| TokenUsageRaw{input_tokens:0,output_tokens:o,cache_creation_input_tokens:0,cache_read_input_tokens:0,speed:None,cache_creation:None};
    // main: A (parent) -> B (child). A owns a turn out=10, B owns out=20.
    let main = Thread { id:"s".into(), is_subagent:false, skills: vec![
        SkillFrame{name:"A".into(),parent:None,body:String::new(),spawned_subagents:vec!["a1".into()]},
        SkillFrame{name:"B".into(),parent:Some(0),body:String::new(),spawned_subagents:vec![]},
    ], turns: vec![ Turn{skill:Some(0),usage:u(10)}, Turn{skill:Some(1),usage:u(20)} ]};
    // subagent thread spawned by A: one baseline turn out=100
    let sub = Thread { id:"sub".into(), is_subagent:true, skills: vec![], turns: vec![ Turn{skill:None,usage:u(100)} ]};
    let graph = ThreadGraph { threads: vec![main, sub], links: vec![SubagentLink{parent_thread:0,parent_skill:Some(0),child_thread:1}] };

    let attr = attribute(&graph);
    let a = attr.skills.iter().find(|s| s.name=="A").unwrap();
    let b = attr.skills.iter().find(|s| s.name=="B").unwrap();
    assert_eq!(b.exclusive.output, 20.0);
    assert_eq!(b.inclusive.output, 20.0);
    // A inclusive = A(10) + B(20) + subagent(100) = 130
    assert_eq!(a.exclusive.output, 10.0);
    assert_eq!(a.inclusive.output, 130.0);
}
```

- [ ] **Step 2: Run to confirm fail.** Run: `cargo test -p ccusage skills::attribute::tests::inclusive_rolls_up`. Expected: FAIL.

- [ ] **Step 3: Implement** in `attribute.rs`:

```rust
use super::thread::ThreadGraph;

#[derive(Debug, Clone)]
pub(crate) struct SkillCost {
    pub(crate) thread: usize,
    pub(crate) skill: usize,
    pub(crate) name: String,
    pub(crate) exclusive: Cost,
    pub(crate) inclusive: Cost,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Attribution {
    pub(crate) skills: Vec<SkillCost>,
    pub(crate) baseline: Cost,
}

fn add_into(a: &mut Cost, b: &Cost) {
    a.input += b.input; a.output += b.output;
    a.cache_creation += b.cache_creation; a.cache_read += b.cache_read;
}

pub(crate) fn attribute(graph: &ThreadGraph) -> Attribution {
    // exclusive per (thread, skill) and per-thread baseline
    let mut excl: Vec<Vec<Cost>> = Vec::with_capacity(graph.threads.len());
    let mut thread_base: Vec<Cost> = Vec::with_capacity(graph.threads.len());
    for t in &graph.threads {
        let (per, base) = exclusive_per_thread(t);
        excl.push(per);
        thread_base.push(base);
    }

    // full cost of an entire thread (all skills + baseline) — used for subagent roll-up
    let thread_total = |ti: usize| -> Cost {
        let mut c = thread_base[ti];
        for sc in &excl[ti] { add_into(&mut c, sc); }
        c
    };

    // inclusive within each thread: post-order over parent links
    let mut incl = excl.clone();
    for (ti, t) in graph.threads.iter().enumerate() {
        // children-of map
        let order: Vec<usize> = topo_children_last(&t.skills);
        for &si in &order {
            if let Some(p) = t.skills[si].parent {
                let child_incl = incl[ti][si];
                add_into(&mut incl[ti][p], &child_incl);
            }
        }
    }

    // subagent threads roll into the linking skill's inclusive (and its ancestors via the
    // already-computed within-thread inclusive: add child thread total to the linked skill,
    // then re-propagate to ancestors).
    for link in &graph.links {
        let child_total = thread_total(link.child_thread);
        match link.parent_skill {
            Some(ps) => {
                add_into(&mut incl[link.parent_thread][ps], &child_total);
                // propagate to ancestors of ps
                let mut cur = graph.threads[link.parent_thread].skills[ps].parent;
                while let Some(a) = cur {
                    add_into(&mut incl[link.parent_thread][a], &child_total);
                    cur = graph.threads[link.parent_thread].skills[a].parent;
                }
            }
            None => { /* subagent spawned with no active skill: stays in baseline */ }
        }
    }

    let mut out = Attribution::default();
    out.baseline = thread_base[0];
    for (ti, t) in graph.threads.iter().enumerate() {
        for (si, frame) in t.skills.iter().enumerate() {
            out.skills.push(SkillCost {
                thread: ti, skill: si, name: frame.name.clone(),
                exclusive: excl[ti][si], inclusive: incl[ti][si],
            });
        }
    }
    out
}

/// Children-before-parents order so a child's inclusive is final before folding into its parent.
fn topo_children_last(skills: &[super::thread::SkillFrame]) -> Vec<usize> {
    // depth = chain length to root; deeper first
    let depth = |mut i: usize| -> usize {
        let mut d = 0;
        while let Some(p) = skills[i].parent { i = p; d += 1; }
        d
    };
    let mut idx: Vec<usize> = (0..skills.len()).collect();
    idx.sort_by_key(|&i| std::cmp::Reverse(depth(i)));
    idx
}
```

- [ ] **Step 4: Run to confirm pass.** Run: `cargo test -p ccusage skills::attribute`. Expected: PASS (3 tests).

- [ ] **Step 5: Commit.**

```bash
git add rust/crates/ccusage/src/adapter/claude/skills/attribute.rs
git commit -m "feat(skills): inclusive roll-up over nesting and subagents"
```

---

### Task 7: Dimensions (high-context / subagent / long-session / plugin)

**Files:**
- Create: `rust/crates/ccusage/src/adapter/claude/skills/dimensions.rs`

**Interfaces:**
- Consumes: `Record` (Task 1) — dimensions are computed over the deduped record stream + per-file spans, independent of attribution.
- Produces:
  - `struct Dimensions { pub high_context_share: f64, pub subagent_share: f64, pub plugin_tokens: Vec<(String, f64)> }`
  - `fn dimensions(records: &[Record], skills: &[SkillCost]) -> Dimensions`
  - `fn long_session_share(session_totals: &[(f64 /*span_hours*/, f64 /*tokens*/)], min_hours: f64) -> f64`

- [ ] **Step 1: Write the failing tests:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::TokenUsageRaw;
    use crate::adapter::claude::skills::record::{Record, RecordKind};
    use crate::adapter::claude::skills::attribute::{Cost, SkillCost};

    fn rec(inp:u64,cc:u64,cr:u64,side:bool)->Record{
        Record{kind:RecordKind::Assistant,timestamp:None,message_id:None,request_id:None,
            is_sidechain:side,is_meta:false,compact:false,
            usage:Some(TokenUsageRaw{input_tokens:inp,output_tokens:0,cache_creation_input_tokens:cc,cache_read_input_tokens:cr,speed:None,cache_creation:None}),
            model:None,blocks:vec![]}
    }

    #[test]
    fn high_context_share_is_token_weighted_over_150k() {
        // one small request (context 100), one big (context 200_000)
        let recs = vec![ rec(100,0,0,false), rec(0,0,200_000,false) ];
        let d = dimensions(&recs, &[]);
        // big request tokens / total = 200000 / 200100
        assert!((d.high_context_share - 200_000.0/200_100.0).abs() < 1e-9);
    }

    #[test]
    fn subagent_share_is_sidechain_token_fraction() {
        let recs = vec![ rec(100,0,0,false), rec(300,0,0,true) ];
        let d = dimensions(&recs, &[]);
        assert!((d.subagent_share - 300.0/400.0).abs() < 1e-9);
    }

    #[test]
    fn plugin_groups_by_namespace_prefix() {
        let sc = |n:&str,o:f64| SkillCost{thread:0,skill:0,name:n.into(),
            exclusive:Cost{input:0.0,output:o,cache_creation:0.0,cache_read:0.0},
            inclusive:Cost{input:0.0,output:o,cache_creation:0.0,cache_read:0.0}};
        let skills = vec![ sc("superpowers:brainstorming",10.0), sc("superpowers:writing-plans",5.0), sc("air-skills:ci-triage",2.0) ];
        let d = dimensions(&[], &skills);
        let sp = d.plugin_tokens.iter().find(|(p,_)| p=="superpowers").unwrap().1;
        assert_eq!(sp, 15.0);
    }

    #[test]
    fn long_session_share_thresholds_on_span() {
        // (span_hours, tokens): two sessions, one 9h/900 one 1h/100
        let share = long_session_share(&[(9.0,900.0),(1.0,100.0)], 8.0);
        assert!((share - 900.0/1000.0).abs() < 1e-9);
    }
}
```

- [ ] **Step 2: Run to confirm fail.** Run: `cargo test -p ccusage skills::dimensions`. Expected: FAIL.

- [ ] **Step 3: Implement `dimensions.rs`:**

```rust
use std::collections::BTreeMap;

use super::attribute::SkillCost;
use super::record::Record;

#[derive(Debug, Clone, Default)]
pub(crate) struct Dimensions {
    pub(crate) high_context_share: f64,
    pub(crate) subagent_share: f64,
    pub(crate) plugin_tokens: Vec<(String, f64)>,
}

const HIGH_CONTEXT: f64 = 150_000.0;

pub(crate) fn dimensions(records: &[Record], skills: &[SkillCost]) -> Dimensions {
    let mut total = 0.0_f64;
    let mut high = 0.0_f64;
    let mut side = 0.0_f64;
    for r in records {
        let Some(u) = r.usage else { continue };
        let ctx = (u.input_tokens + u.cache_creation_token_count() + u.cache_read_input_tokens) as f64;
        let tok = ctx + u.output_tokens as f64;
        total += tok;
        if ctx > HIGH_CONTEXT { high += tok; }
        if r.is_sidechain { side += tok; }
    }
    let mut plugins: BTreeMap<String, f64> = BTreeMap::new();
    for s in skills {
        let plugin = s.name.split(':').next().unwrap_or("").to_string();
        *plugins.entry(plugin).or_default() += s.exclusive.total();
    }
    let mut plugin_tokens: Vec<(String, f64)> = plugins.into_iter().collect();
    plugin_tokens.sort_by(|a, b| b.1.total_cmp(&a.1));

    Dimensions {
        high_context_share: if total > 0.0 { high / total } else { 0.0 },
        subagent_share: if total > 0.0 { side / total } else { 0.0 },
        plugin_tokens,
    }
}

pub(crate) fn long_session_share(session_totals: &[(f64, f64)], min_hours: f64) -> f64 {
    let total: f64 = session_totals.iter().map(|(_, t)| t).sum();
    if total == 0.0 { return 0.0; }
    let long: f64 = session_totals.iter().filter(|(h, _)| *h >= min_hours).map(|(_, t)| t).sum();
    long / total
}
```

- [ ] **Step 4: Run to confirm pass.** Run: `cargo test -p ccusage skills::dimensions`. Expected: PASS (4 tests).

- [ ] **Step 5: Commit.**

```bash
git add rust/crates/ccusage/src/adapter/claude/skills/dimensions.rs
git commit -m "feat(skills): usage characteristic dimensions"
```

---

### Task 8: Report struct + cost pricing + JSON/table rendering

**Files:**
- Create: `rust/crates/ccusage/src/adapter/claude/skills/report.rs`

**Interfaces:**
- Consumes: `Attribution`, `Cost` (Tasks 5-6), `Dimensions` (Task 7), `crate::calculate_cost_for_usage`, `crate::PricingMap`.
- Produces:
  - `fn price(cost: &Cost, model: &str, pricing: Option<&PricingMap>) -> f64`
  - `struct SkillsReport { ... }` with `serde::Serialize`
  - `fn build_report(attr: &Attribution, dims: &Dimensions, model: &str, pricing: Option<&PricingMap>) -> SkillsReport`
  - `fn report_json(report: &SkillsReport) -> serde_json::Value`

Pricing a split `Cost`: call `calculate_cost_for_usage` four times, each with a `TokenUsageRaw` carrying only one component (rounded to u64), summing. This reuses ccusage's per-component rates without duplicating them.

- [ ] **Step 1: Write the failing test:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::claude::skills::attribute::{Attribution, Cost, SkillCost};
    use crate::adapter::claude::skills::dimensions::Dimensions;

    #[test]
    fn report_is_serializable_and_sorted_by_inclusive() {
        let big = SkillCost{thread:0,skill:0,name:"A".into(),
            exclusive:Cost{input:0.0,output:10.0,cache_creation:0.0,cache_read:0.0},
            inclusive:Cost{input:0.0,output:100.0,cache_creation:0.0,cache_read:0.0}};
        let small = SkillCost{thread:0,skill:1,name:"B".into(),
            exclusive:Cost{input:0.0,output:5.0,cache_creation:0.0,cache_read:0.0},
            inclusive:Cost{input:0.0,output:5.0,cache_creation:0.0,cache_read:0.0}};
        let attr = Attribution{ skills: vec![small, big], baseline: Cost::default() };
        let dims = Dimensions::default();
        let rep = build_report(&attr, &dims, "claude-opus-4-8", None);
        assert_eq!(rep.skills[0].name, "A"); // sorted by inclusive desc
        let v = report_json(&rep);
        assert!(v.get("skills").is_some());
        serde_json::to_string(&rep).unwrap();
    }
}
```

- [ ] **Step 2: Run to confirm fail.** Run: `cargo test -p ccusage skills::report`. Expected: FAIL.

- [ ] **Step 3: Implement `report.rs`:**

```rust
use serde::Serialize;

use crate::{PricingMap, TokenUsageRaw, calculate_cost_for_usage};
use crate::cli::CostMode;

use super::attribute::{Attribution, Cost};
use super::dimensions::Dimensions;

fn one(field: impl FnOnce(&mut TokenUsageRaw)) -> TokenUsageRaw {
    let mut u = TokenUsageRaw { input_tokens:0, output_tokens:0,
        cache_creation_input_tokens:0, cache_read_input_tokens:0, speed:None, cache_creation:None };
    field(&mut u);
    u
}

pub(crate) fn price(cost: &Cost, model: &str, pricing: Option<&PricingMap>) -> f64 {
    let m = Some(model);
    let calc = |u: TokenUsageRaw| calculate_cost_for_usage(m, u, None, CostMode::Calculate, pricing);
    calc(one(|u| u.input_tokens = cost.input.round() as u64))
        + calc(one(|u| u.output_tokens = cost.output.round() as u64))
        + calc(one(|u| u.cache_creation_input_tokens = cost.cache_creation.round() as u64))
        + calc(one(|u| u.cache_read_input_tokens = cost.cache_read.round() as u64))
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SkillRow {
    pub(crate) name: String,
    pub(crate) exclusive_tokens: u64,
    pub(crate) inclusive_tokens: u64,
    pub(crate) exclusive_cost: f64,
    pub(crate) inclusive_cost: f64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SkillsReport {
    pub(crate) model: String,
    pub(crate) skills: Vec<SkillRow>,
    pub(crate) baseline_tokens: u64,
    pub(crate) high_context_share: f64,
    pub(crate) subagent_share: f64,
    pub(crate) plugin_tokens: Vec<(String, f64)>,
}

pub(crate) fn build_report(attr: &Attribution, dims: &Dimensions, model: &str, pricing: Option<&PricingMap>) -> SkillsReport {
    let mut rows: Vec<SkillRow> = attr.skills.iter().map(|s| SkillRow {
        name: s.name.clone(),
        exclusive_tokens: s.exclusive.total().round() as u64,
        inclusive_tokens: s.inclusive.total().round() as u64,
        exclusive_cost: price(&s.exclusive, model, pricing),
        inclusive_cost: price(&s.inclusive, model, pricing),
    }).collect();
    rows.sort_by(|a, b| b.inclusive_tokens.cmp(&a.inclusive_tokens));
    SkillsReport {
        model: model.to_string(),
        skills: rows,
        baseline_tokens: attr.baseline.total().round() as u64,
        high_context_share: dims.high_context_share,
        subagent_share: dims.subagent_share,
        plugin_tokens: dims.plugin_tokens.clone(),
    }
}

pub(crate) fn report_json(report: &SkillsReport) -> serde_json::Value {
    serde_json::to_value(report).unwrap_or(serde_json::Value::Null)
}
```

> Verify `CostMode::Calculate` is the variant name (`cli.rs` — `daily.rs:38` uses `CostMode::Display`). If the enum differs, use the calculate-from-tokens variant.

- [ ] **Step 4: Run to confirm pass.** Run: `cargo test -p ccusage skills::report`. Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add rust/crates/ccusage/src/adapter/claude/skills/report.rs
git commit -m "feat(skills): report struct, per-component pricing, JSON"
```

---

### Task 9: CLI command wiring + file loading

**Files:**
- Modify: `rust/crates/ccusage/src/adapter/claude/skills/mod.rs` (add `run_skills`)
- Modify: `rust/crates/ccusage/src/cli.rs` (add `SkillsArgs` + `Command::Skills` — mirror `SessionArgs`/`Command::Session`)
- Modify: `rust/crates/ccusage/src/main.rs:~132` (dispatch)
- Modify: `rust/crates/ccusage-cli/src/parser.rs` (register the `skills` subcommand — mirror `session`)

**Interfaces:**
- Consumes: all prior modules.
- Produces: `pub(crate) fn run_skills(args: crate::cli::SkillsArgs) -> crate::Result<()>`.

- [ ] **Step 1: Read the template.** Read `commands::run_session` (`commands/mod.rs:144-215`), `run_session_id` (`:217-268`), and how `SessionArgs`/`Command::Session` are declared in `cli.rs` and parsed in `ccusage-cli/src/parser.rs`. The new command copies this structure: a `SharedArgs` (for `--since/--until/--json/--jq/--timezone/--offline`) plus a `--min-hours` flag (default 8) for the long-session dimension.

- [ ] **Step 2: Add `SkillsArgs` + `Command::Skills`** in `cli.rs`, mirroring `SessionArgs`:

```rust
#[derive(Debug, Clone)]
pub(crate) struct SkillsArgs {
    pub(crate) shared: SharedArgs,
    pub(crate) min_hours: f64,
}
// add to the Command enum:
//   Skills(SkillsArgs),
```

- [ ] **Step 3: Register the subcommand** in `ccusage-cli/src/parser.rs` by copying the `session` arm; map `--min-hours` to `SkillsArgs.min_hours` (default `8.0`). Add `Some(Command::Skills(args)) => commands::run_skills(args),`? No — keep adapter-scoped: dispatch directly in `main.rs`:

```rust
Some(Command::Skills(args)) => adapter::claude::skills::run_skills(args),
```

- [ ] **Step 4: Implement `run_skills`** in `skills/mod.rs`:

```rust
use std::fs;

use crate::adapter::claude::paths::{claude_paths, usage_files};
use crate::cli::SkillsArgs;
use crate::{PricingMap, Result, output};

use self::attribute::attribute;
use self::dimensions::{dimensions, long_session_share};
use self::dedup::dedup;
use self::record::parse_records;
use self::report::{build_report, report_json};
use self::thread::{build_thread, link_subagents};

pub(crate) fn run_skills(args: SkillsArgs) -> Result<()> {
    let paths = claude_paths()?;
    let files = usage_files(&paths, None);

    // Partition: main-session files vs subagent files (path contains `/subagents/`).
    let mut main_files = Vec::new();
    let mut sub_files = Vec::new();
    for f in files {
        if f.components().any(|c| c.as_os_str() == "subagents") {
            sub_files.push(f);
        } else {
            main_files.push(f);
        }
    }

    let pricing = Some(PricingMap::load_with_overrides(args.shared.offline, false, std::iter::empty()));

    // For v1 attribution we process the newest main session and its subagents; the
    // dimensions span all files. (Multi-session aggregation is a follow-up.)
    let mut all_records = Vec::new();
    let mut graphs = Vec::new();
    let mut session_totals: Vec<(f64, f64)> = Vec::new();

    for mf in &main_files {
        let Ok(content) = fs::read(mf) else { continue };
        let recs = dedup(parse_records(&content));
        // session span + tokens for long-session dimension
        let (span, toks) = span_and_tokens(&recs);
        session_totals.push((span, toks));
        all_records.extend(recs.iter().cloned());

        let main_thread = build_thread(&recs, mf.to_string_lossy().into_owned(), false);
        // subagent files whose path shares this session's stem
        let stem = mf.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let mut subs = Vec::new();
        for sf in &sub_files {
            if sf.to_string_lossy().contains(stem) {
                if let Ok(c) = fs::read(sf) {
                    let sr = dedup(parse_records(&c));
                    all_records.extend(sr.iter().cloned());
                    subs.push((sf.to_string_lossy().into_owned(), build_thread(&sr, sf.to_string_lossy().into_owned(), true)));
                }
            }
        }
        graphs.push(link_subagents(main_thread, subs));
    }

    // attribute across graphs (merge skill lists)
    let mut merged = crate::adapter::claude::skills::attribute::Attribution::default();
    for g in &graphs {
        let a = attribute(g);
        merged.skills.extend(a.skills);
        merged.baseline.input += a.baseline.input;
        merged.baseline.output += a.baseline.output;
        merged.baseline.cache_creation += a.baseline.cache_creation;
        merged.baseline.cache_read += a.baseline.cache_read;
    }

    let model = all_records.iter().rev().find_map(|r| r.model.clone())
        .unwrap_or_else(|| "claude-opus-4-8".to_string());
    let mut dims = dimensions(&all_records, &merged.skills);
    let _long = long_session_share(&session_totals, args.min_hours); // included in print below

    let report = build_report(&merged, &dims, &model, pricing.as_ref());

    if output::wants_json(&args.shared) {
        output::print_json_or_jq(report_json(&report), args.shared.jq.as_deref(), args.shared.no_cost)?;
    } else {
        print_table(&report, _long);
    }
    let _ = &mut dims;
    Ok(())
}

fn span_and_tokens(recs: &[record::Record]) -> (f64, f64) {
    let mut toks = 0.0;
    let mut first: Option<String> = None;
    let mut last: Option<String> = None;
    for r in recs {
        if let Some(u) = r.usage {
            toks += (u.input_tokens + u.output_tokens + u.cache_creation_token_count() + u.cache_read_input_tokens) as f64;
        }
        if let Some(ts) = &r.timestamp {
            if first.is_none() { first = Some(ts.clone()); }
            last = Some(ts.clone());
        }
    }
    let span = match (first, last) {
        (Some(a), Some(b)) => crate::parse_ts_timestamp(&b).zip(crate::parse_ts_timestamp(&a))
            .map(|(b, a)| (b - a) as f64 / 3_600_000.0).unwrap_or(0.0),
        _ => 0.0,
    };
    (span, toks)
}

fn print_table(report: &report::SkillsReport, long_share: f64) {
    crate::print_box_title("Claude Code Skill Attribution");
    println!("model: {}", report.model);
    println!("{:<40} {:>14} {:>14} {:>10} {:>10}", "skill", "excl.tokens", "incl.tokens", "excl.$", "incl.$");
    for s in &report.skills {
        println!("{:<40} {:>14} {:>14} {:>10.4} {:>10.4}",
            truncate(&s.name, 40), s.exclusive_tokens, s.inclusive_tokens, s.exclusive_cost, s.inclusive_cost);
    }
    println!("baseline (no skill): {} tokens", report.baseline_tokens);
    println!();
    println!("high-context (>150k) share: {:.1}%", report.high_context_share * 100.0);
    println!("subagent share:             {:.1}%", report.subagent_share * 100.0);
    println!("long-session (>=8h) share:  {:.1}%", long_share * 100.0);
    println!("\nby plugin:");
    for (p, t) in &report.plugin_tokens {
        println!("  {:<24} {:>14.0}", p, t);
    }
    println!("\nnote: inclusive double-counts by design (nesting + subagents); exclusive reconciles to billed.");
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n { s.to_string() } else { s.chars().take(n).collect() }
}
```

> `print_box_title` and `parse_ts_timestamp` are existing crate helpers (used across `commands/mod.rs` and `adapter/claude/paths.rs`). Confirm exact paths/signatures while implementing; adjust imports.

- [ ] **Step 5: Build and smoke-run.** Run: `cargo build -p ccusage` (expected: clean build), then `cargo run -p ccusage -- skills --json | head -40` (expected: JSON with `skills`/`high_context_share`).

- [ ] **Step 6: Commit.**

```bash
git add rust/crates/ccusage/src/adapter/claude/skills/mod.rs rust/crates/ccusage/src/cli.rs rust/crates/ccusage/src/main.rs rust/crates/ccusage-cli/src/parser.rs
git commit -m "feat(skills): wire skills command, file loading, table+json output"
```

---

### Task 10: Corpus invariants + oracle cross-check

**Files:**
- Create: `rust/crates/ccusage/src/adapter/claude/skills/tests_corpus.rs` (gated behind an env var so CI without local data skips it)
- Modify: `skills/mod.rs` (add `#[cfg(test)] mod tests_corpus;`)

**Interfaces:**
- Consumes: the full pipeline.

- [ ] **Step 1: Write the invariant test** (runs only when `CCUSAGE_CORPUS=1` and local data exists):

```rust
#[cfg(test)]
mod tests_corpus {
    use super::*;
    use crate::adapter::claude::paths::{claude_paths, usage_files};
    use std::fs;

    fn enabled() -> bool { std::env::var("CCUSAGE_CORPUS").is_ok() }

    #[test]
    fn exclusive_reconciles_and_inclusive_dominates() {
        if !enabled() { return; }
        let paths = claude_paths().unwrap_or_default();
        let files = usage_files(&paths, None);
        for f in files.into_iter().take(50) {
            if f.components().any(|c| c.as_os_str() == "subagents") { continue; }
            let Ok(content) = fs::read(&f) else { continue };
            let recs = super::dedup::dedup(super::record::parse_records(&content));
            let t = super::thread::build_thread(&recs, f.to_string_lossy().into(), false);
            let graph = super::thread::link_subagents(t, vec![]);
            let attr = super::attribute::attribute(&graph);

            // invariant 3: inclusive >= exclusive
            for s in &attr.skills {
                assert!(s.inclusive.total() + 1.0 >= s.exclusive.total(), "incl<excl in {:?}", f);
            }
            // invariant 1: exclusive partition <= billed (no over-attribution)
            let billed: f64 = recs.iter().filter_map(|r| r.usage).map(|u|
                (u.input_tokens + u.output_tokens + u.cache_creation_token_count() + u.cache_read_input_tokens) as f64).sum();
            let attributed: f64 = attr.skills.iter().map(|s| s.exclusive.total()).sum::<f64>() + attr.baseline.total();
            assert!(attributed <= billed * 1.001 + 1.0, "over-attributed in {:?}: {} > {}", f, attributed, billed);
        }
    }
}
```

- [ ] **Step 2: Run without corpus (should pass trivially).** Run: `cargo test -p ccusage skills::tests_corpus`. Expected: PASS (returns early).

- [ ] **Step 3: Run with corpus.** Run: `CCUSAGE_CORPUS=1 cargo test -p ccusage skills::tests_corpus -- --nocapture`. Expected: PASS on local transcripts.

- [ ] **Step 4: Oracle cross-check (manual, documented).** Run the Python oracle and the new command on the same single transcript; confirm exclusive per-skill token totals agree within a small tolerance (the oracle currently models body-only; extend its rent function to the apportioned model first, or compare only the components both compute — output + cache_creation per skill):

```bash
python3 ../skill-token-usage-estimator/token_estimator.py <T>.jsonl --json > /tmp/oracle.json
cargo run -p ccusage -- skills --json > /tmp/rust.json
# diff the per-skill output+cache_creation exclusive figures
```

- [ ] **Step 5: Commit.**

```bash
git add rust/crates/ccusage/src/adapter/claude/skills/tests_corpus.rs rust/crates/ccusage/src/adapter/claude/skills/mod.rs
git commit -m "test(skills): corpus invariants and oracle cross-check"
```

---

## Self-Review

**Spec coverage:**
- Fork + claude-first → Tasks 1-10 all in the fork, claude adapter. ✓
- Second content-pass on loader → Task 1 (`parse_records` reads content, not usage-prefiltered). ✓
- Port: dedup+sidechain (Task 2), recursive discovery + subagent files (Task 9 via `usage_files`/`claude_paths` — already recursive), structured cache (used everywhere via `cache_creation_token_count`), validity filters (partial — see gap), agentProgress (noted Task 1 follow-up). ◑
- Threads/stack/boundary(b)/nest(C) → Task 3. ✓
- Subagent attribution → Tasks 4, 6, 9. ✓
- Exclusive + cr apportionment → Task 5. ✓
- Inclusive roll-up → Task 6. ✓
- Dimensions → Task 7. ✓
- Reconciliation invariants → Task 10. ✓
- Oracle lock-step → Task 10 step 4. ✓

**Gaps found (addressed inline):**
- *Validity filters* (drop empty ids, `<synthetic>`, non-semver) from `daily.rs:317-356` aren't a dedicated task. They matter mostly for cost accuracy, less for attribution. **Action:** fold a `is_valid` filter into `dedup` (Task 2) as a follow-up step — drop records whose `message_id`/`request_id` are empty strings and whose model is `<synthetic>`. Low risk; not blocking.
- *agentProgress-wrapped usage* — Task 1 note covers it; add the untagged enum only if the corpus run (Task 10) shows missed sidechain usage.
- *Multi-session attribution aggregation* — Task 9 processes per-main-session and merges skill rows; same-named skills across sessions appear as separate rows. **Action:** acceptable for v1; a grouping pass (sum by skill name) is a trivial follow-up if the report is noisy.

**Placeholder scan:** no TBD/"handle errors"/undefined refs. All ccusage helpers referenced (`calculate_cost_for_usage`, `PricingMap`, `TokenUsageRaw`, `cache_creation_token_count`, `claude_paths`, `usage_files`, `output::*`, `print_box_title`, `parse_ts_timestamp`) are real, cited to source; where a signature wasn't fully read, the step says to confirm/mirror the existing `session` command.

**Type consistency:** `Cost`, `SkillCost`, `Attribution`, `Thread`, `SkillFrame`, `ThreadGraph`, `SubagentLink`, `Dimensions`, `SkillsReport` names match across Tasks 3-9. `cache_creation_token_count()` used consistently for cc. Owner index convention (`Some(i)` skill, `None`/slot `n` baseline) consistent Tasks 3/5.

## Known approximations (carried from spec risks)
- cr apportionment uses `prefix ≈ Σ cc`, ignoring eviction — reconciles to bill, may skew very long sessions.
- Subagent linkage by spawn order (Task 4) — replace with id-join if the transcript exposes the parent tool_use id (re-check in Task 10).
- (C) nesting via body substring match — false positives possible; mitigate later by requiring the referenced skill to actually be invoked (it is, since we only nest on real invocations).
