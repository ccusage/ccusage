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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TokenUsageRaw;
    use crate::adapter::claude::skills::record::{Record, RecordKind};
    use crate::adapter::claude::skills::attribute::{Cost, SkillCost};

    fn rec(inp: u64, cc: u64, cr: u64, side: bool) -> Record {
        Record {
            kind: RecordKind::Assistant,
            timestamp: None,
            message_id: None,
            request_id: None,
            is_sidechain: side,
            is_meta: false,
            compact: false,
            usage: Some(TokenUsageRaw { input_tokens: inp, output_tokens: 0, cache_creation_input_tokens: cc, cache_read_input_tokens: cr, speed: None, cache_creation: None }),
            model: None,
            blocks: vec![],
        }
    }

    #[test]
    fn high_context_share_is_token_weighted_over_150k() {
        // one small request (context 100), one big (context 200_000)
        let recs = vec![rec(100, 0, 0, false), rec(0, 0, 200_000, false)];
        let d = dimensions(&recs, &[]);
        // big request tokens / total = 200000 / 200100
        assert!((d.high_context_share - 200_000.0 / 200_100.0).abs() < 1e-9);
    }

    #[test]
    fn subagent_share_is_sidechain_token_fraction() {
        let recs = vec![rec(100, 0, 0, false), rec(300, 0, 0, true)];
        let d = dimensions(&recs, &[]);
        assert!((d.subagent_share - 300.0 / 400.0).abs() < 1e-9);
    }

    #[test]
    fn plugin_groups_by_namespace_prefix() {
        let sc = |n: &str, o: f64| SkillCost {
            thread: 0,
            skill: 0,
            name: n.into(),
            exclusive: Cost { input: 0.0, output: o, cache_creation: 0.0, cache_read: 0.0 },
            inclusive: Cost { input: 0.0, output: o, cache_creation: 0.0, cache_read: 0.0 },
        };
        let skills = vec![sc("superpowers:brainstorming", 10.0), sc("superpowers:writing-plans", 5.0), sc("air-skills:ci-triage", 2.0)];
        let d = dimensions(&[], &skills);
        let sp = d.plugin_tokens.iter().find(|(p, _)| p == "superpowers").unwrap().1;
        assert_eq!(sp, 15.0);
    }

    #[test]
    fn long_session_share_thresholds_on_span() {
        // (span_hours, tokens): two sessions, one 9h/900 one 1h/100
        let share = long_session_share(&[(9.0, 900.0), (1.0, 100.0)], 8.0);
        assert!((share - 900.0 / 1000.0).abs() < 1e-9);
    }
}
