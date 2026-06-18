use super::thread::{Thread, ThreadGraph};

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

/// Returns exclusive cost per skill (indexed like `thread.skills`) and the baseline cost.
///
/// `out`, `cc`, and `in` go to the owning skill directly. `cache_read` is apportioned across
/// all skills proportionally to the cumulative `cache_creation` each has injected so far.
pub(crate) fn exclusive_per_thread(thread: &Thread) -> (Vec<Cost>, Cost) {
    let n = thread.skills.len();
    let mut per = vec![Cost::default(); n];
    let mut base = Cost::default();
    // cumulative cache_creation injected per slot; slot n == baseline
    let mut added = vec![0.0_f64; n + 1];
    let mut prefix = 0.0_f64;

    for turn in &thread.turns {
        let owner = turn.skill.unwrap_or(n);
        let usage = turn.usage;
        let cr = usage.cache_read_input_tokens as f64;

        if cr > 0.0 && prefix > 0.0 {
            for slot in 0..=n {
                if added[slot] > 0.0 {
                    let share = cr * (added[slot] / prefix);
                    if slot == n {
                        base.cache_read += share;
                    } else {
                        per[slot].cache_read += share;
                    }
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
    a.input += b.input;
    a.output += b.output;
    a.cache_creation += b.cache_creation;
    a.cache_read += b.cache_read;
}

pub(crate) fn attribute(graph: &ThreadGraph) -> Attribution {
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
        for sc in &excl[ti] {
            add_into(&mut c, sc);
        }
        c
    };

    // inclusive within each thread: children-before-parents so child inclusive is final
    let mut incl = excl.clone();
    for (ti, t) in graph.threads.iter().enumerate() {
        let order = topo_children_last(&t.skills);
        for &si in &order {
            if let Some(p) = t.skills[si].parent {
                let child_incl = incl[ti][si];
                add_into(&mut incl[ti][p], &child_incl);
            }
        }
    }

    // subagent threads roll into the linking skill's inclusive, then propagate to ancestors
    for link in &graph.links {
        let child_total = thread_total(link.child_thread);
        if let Some(ps) = link.parent_skill {
            add_into(&mut incl[link.parent_thread][ps], &child_total);
            let mut cur = graph.threads[link.parent_thread].skills[ps].parent;
            while let Some(a) = cur {
                add_into(&mut incl[link.parent_thread][a], &child_total);
                cur = graph.threads[link.parent_thread].skills[a].parent;
            }
        }
        // if parent_skill is None the subagent has no owning skill; stays in baseline only
    }

    let mut out = Attribution::default();
    out.baseline = thread_base[0];
    for (ti, t) in graph.threads.iter().enumerate() {
        for (si, frame) in t.skills.iter().enumerate() {
            out.skills.push(SkillCost {
                thread: ti,
                skill: si,
                name: frame.name.clone(),
                exclusive: excl[ti][si],
                inclusive: incl[ti][si],
            });
        }
    }
    out
}

/// Children-before-parents order so a child's inclusive is final before folding into its parent.
fn topo_children_last(skills: &[super::thread::SkillFrame]) -> Vec<usize> {
    let depth = |mut i: usize| -> usize {
        let mut d = 0;
        while let Some(p) = skills[i].parent {
            i = p;
            d += 1;
        }
        d
    };
    let mut idx: Vec<usize> = (0..skills.len()).collect();
    idx.sort_by_key(|&i| std::cmp::Reverse(depth(i)));
    idx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TokenUsageRaw;
    use crate::adapter::claude::skills::thread::{SkillFrame, Thread, Turn};

    fn u(inp: u64, out: u64, cc: u64, cr: u64) -> TokenUsageRaw {
        TokenUsageRaw {
            input_tokens: inp,
            output_tokens: out,
            cache_creation_input_tokens: cc,
            cache_read_input_tokens: cr,
            speed: None,
            cache_creation: None,
        }
    }

    fn frame(n: &str) -> SkillFrame {
        SkillFrame { name: n.into(), parent: None, body: String::new(), spawned_subagents: vec![] }
    }

    #[test]
    fn apportions_cache_read_by_injected_cc_share() {
        // turn0: skill A owns, cc=100, cr=0
        // turn1: skill B owns, cc=100, cr=200 -> prefix_before=100 (all A) => all 200 cr to A
        // turn2: skill B owns, cc=0,   cr=400 -> prefix=200 (A=100,B=100) => 200 to A, 200 to B
        let t = Thread {
            id: "s".into(),
            is_subagent: false,
            skills: vec![frame("A"), frame("B")],
            turns: vec![
                Turn { skill: Some(0), usage: u(0, 0, 100, 0) },
                Turn { skill: Some(1), usage: u(0, 0, 100, 200) },
                Turn { skill: Some(1), usage: u(0, 0, 0, 400) },
            ],
        };
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
    fn inclusive_rolls_up_nested_and_subagent_costs() {
        use crate::adapter::claude::skills::thread::{SkillFrame, SubagentLink, Thread, ThreadGraph, Turn};
        let u = |o: u64| TokenUsageRaw {
            input_tokens: 0,
            output_tokens: o,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            speed: None,
            cache_creation: None,
        };
        // main: A (parent) -> B (child). A owns a turn out=10, B owns out=20.
        let main = Thread {
            id: "s".into(),
            is_subagent: false,
            skills: vec![
                SkillFrame { name: "A".into(), parent: None, body: String::new(), spawned_subagents: vec!["a1".into()] },
                SkillFrame { name: "B".into(), parent: Some(0), body: String::new(), spawned_subagents: vec![] },
            ],
            turns: vec![Turn { skill: Some(0), usage: u(10) }, Turn { skill: Some(1), usage: u(20) }],
        };
        // subagent thread spawned by A: one baseline turn out=100
        let sub = Thread {
            id: "sub".into(),
            is_subagent: true,
            skills: vec![],
            turns: vec![Turn { skill: None, usage: u(100) }],
        };
        let graph = ThreadGraph {
            threads: vec![main, sub],
            links: vec![SubagentLink { parent_thread: 0, parent_skill: Some(0), child_thread: 1 }],
        };

        let attr = attribute(&graph);
        let a = attr.skills.iter().find(|s| s.name == "A").unwrap();
        let b = attr.skills.iter().find(|s| s.name == "B").unwrap();
        assert_eq!(b.exclusive.output, 20.0);
        assert_eq!(b.inclusive.output, 20.0);
        // A inclusive = A(10) + B(20) + subagent(100) = 130
        assert_eq!(a.exclusive.output, 10.0);
        assert_eq!(a.inclusive.output, 130.0);
    }

    #[test]
    fn baseline_owns_unattributed_turns() {
        let t = Thread {
            id: "s".into(),
            is_subagent: false,
            skills: vec![frame("A")],
            turns: vec![
                Turn { skill: None, usage: u(5, 5, 0, 0) },
                Turn { skill: Some(0), usage: u(1, 1, 0, 0) },
            ],
        };
        let (per, base) = exclusive_per_thread(&t);
        assert_eq!(base.input, 5.0);
        assert_eq!(base.output, 5.0);
        assert_eq!(per[0].input, 1.0);
    }
}
