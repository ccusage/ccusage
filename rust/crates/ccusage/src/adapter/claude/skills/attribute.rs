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
