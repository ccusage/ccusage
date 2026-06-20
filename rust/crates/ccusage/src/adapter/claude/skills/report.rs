use serde::Serialize;

use crate::{TokenUsageRaw, calculate_cost_for_usage};
use crate::cli::CostMode;
use crate::pricing::PricingMap;
use super::attribute::{Attribution, Cost};
use super::dimensions::Dimensions;

fn one(field: impl FnOnce(&mut TokenUsageRaw)) -> TokenUsageRaw {
    let mut u = TokenUsageRaw {
        input_tokens: 0,
        output_tokens: 0,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
        speed: None,
        cache_creation: None,
    };
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
    pub(crate) long_session_share: f64,
    pub(crate) plugin_tokens: Vec<(String, f64)>,
}

pub(crate) fn build_report(
    attr: &Attribution,
    dims: &Dimensions,
    model: &str,
    pricing: Option<&PricingMap>,
) -> SkillsReport {
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
        long_session_share: 0.0,
        plugin_tokens: dims.plugin_tokens.clone(),
    }
}

pub(crate) fn report_json(report: &SkillsReport) -> serde_json::Value {
    serde_json::to_value(report).unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::claude::skills::attribute::{Attribution, Cost, SkillCost};
    use crate::adapter::claude::skills::dimensions::Dimensions;

    #[test]
    fn report_is_serializable_and_sorted_by_inclusive() {
        let big = SkillCost {
            thread: 0,
            skill: 0,
            name: "A".into(),
            exclusive: Cost { input: 0.0, output: 10.0, cache_creation: 0.0, cache_read: 0.0 },
            inclusive: Cost { input: 0.0, output: 100.0, cache_creation: 0.0, cache_read: 0.0 },
        };
        let small = SkillCost {
            thread: 0,
            skill: 1,
            name: "B".into(),
            exclusive: Cost { input: 0.0, output: 5.0, cache_creation: 0.0, cache_read: 0.0 },
            inclusive: Cost { input: 0.0, output: 5.0, cache_creation: 0.0, cache_read: 0.0 },
        };
        let attr = Attribution { skills: vec![small, big], baseline: Cost::default() };
        let dims = Dimensions::default();
        let rep = build_report(&attr, &dims, "claude-opus-4-8", None);
        assert_eq!(rep.skills[0].name, "A"); // sorted by inclusive desc
        let v = report_json(&rep);
        assert!(v.get("skills").is_some());
        serde_json::to_string(&rep).unwrap();
    }
}
