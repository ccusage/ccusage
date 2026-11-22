use std::{collections::BTreeMap, sync::Arc};

use crate::{ModelBreakdown, TokenCounts, UsageSummary, cli::DayLabel, fast::FxHashMap};

use super::ClaudeEntry;

pub(super) fn summarize_daily_entries(
    deduped: &[ClaudeEntry],
    group_by_project: bool,
    window: Option<&ccusage_core::ReportWindow>,
) -> Vec<UsageSummary> {
    let includes = |day| window.is_none_or(|window| window.contains_entry_date(day));
    if group_by_project {
        let mut groups = BTreeMap::<(DayLabel, Arc<str>), DailyAccumulator>::new();
        for entry in deduped {
            groups
                .entry((entry.date.into(), Arc::clone(&entry.project)))
                .or_default()
                .add_entry(entry);
        }
        return groups
            .into_iter()
            .filter(|((date, _), _)| includes(date.day()))
            .map(|((date, project), group)| {
                let mut summary = group.into_summary();
                summary.date = Some(date.day().to_string());
                summary.project = Some(project.to_string());
                summary
            })
            .collect();
    }

    let mut groups = BTreeMap::<DayLabel, DailyAccumulator>::new();
    for entry in deduped {
        groups
            .entry(entry.date.into())
            .or_default()
            .add_entry(entry);
    }
    groups
        .into_iter()
        .filter(|(date, _)| includes(date.day()))
        .map(|(key, group)| {
            let mut summary = group.into_summary();
            summary.date = Some(key.day().to_string());
            summary
        })
        .collect()
}

#[derive(Default)]
struct DailyAccumulator {
    counts: TokenCounts,
    cost: f64,
    models: Vec<String>,
    breakdowns: Vec<ModelBreakdown>,
    breakdown_indexes: FxHashMap<String, usize>,
}

impl DailyAccumulator {
    fn add_entry(&mut self, entry: &ClaudeEntry) {
        self.counts.add_usage(entry.usage);
        self.cost += entry.cost;
        if let Some(model) = &entry.model {
            let model = crate::model_aliases::resolve_model_name(model);
            let index = if let Some(index) = self.breakdown_indexes.get(model.as_ref()) {
                *index
            } else {
                let model = model.into_owned();
                let index = self.breakdowns.len();
                self.breakdown_indexes.insert(model.clone(), index);
                self.models.push(model.clone());
                self.breakdowns.push(ModelBreakdown {
                    model_name: model,
                    ..ModelBreakdown::default()
                });
                index
            };
            let breakdown = &mut self.breakdowns[index];
            breakdown.add_usage(&entry.usage);
            breakdown.cost += entry.cost;
            if entry.missing_pricing {
                breakdown.missing_pricing = true;
            }
            breakdown.estimated_pricing |= entry.estimated_pricing;
            breakdown.add_stored_cost(entry.recorded_cost.billed());
        }
    }

    fn into_summary(mut self) -> UsageSummary {
        ModelBreakdown::sort_by_cost_desc(&mut self.breakdowns);
        UsageSummary {
            date: None,
            month: None,
            week: None,
            session_id: None,
            project_path: None,
            last_activity: None,
            first_activity: None,
            input_tokens: self.counts.input_tokens,
            output_tokens: self.counts.output_tokens,
            cache_creation_tokens: self.counts.cache_creation_tokens,
            cache_read_tokens: self.counts.cache_read_tokens,
            extra_total_tokens: 0,
            total_cost: self.cost,
            credits: None,
            message_count: None,
            models_used: self.models,
            model_breakdowns: self.breakdowns,
            project: None,
            versions: None,
        }
    }
}
