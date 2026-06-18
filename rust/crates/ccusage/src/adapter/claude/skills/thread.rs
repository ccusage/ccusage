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
                if let Some(usage) = r.usage {
                    turns.push(Turn { skill: stack.last().copied(), usage });
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
}
