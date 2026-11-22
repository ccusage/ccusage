use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use crate::{
    LoadedEntry, ModelBreakdown, Result, TimestampMs, TokenCounts, UsageSummary,
    cli::{DayLabel, SortOrder, WeekDay},
    cli_error,
    fast::{FxHashMap, FxHashSet},
    format_rfc3339_millis,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyTarget {
    Date,
    Session,
}

pub fn summarize_by_key_target<K, F, M>(
    entries: &[LoadedEntry],
    key_fn: F,
    meta_fn: M,
    target: KeyTarget,
) -> Result<Vec<UsageSummary>>
where
    K: Ord,
    F: Fn(&LoadedEntry) -> K,
    M: Fn(&K) -> (String, Option<String>),
{
    let mut groups: BTreeMap<K, UsageAccumulator> = BTreeMap::new();
    for entry in entries {
        groups.entry(key_fn(entry)).or_default().add_entry(entry);
    }

    let mut rows = Vec::with_capacity(groups.len());
    for (key, group) in groups {
        let (date, project) = meta_fn(&key);
        let mut summary = group.into_summary();
        match target {
            KeyTarget::Date => summary.date = Some(date),
            KeyTarget::Session => summary.session_id = Some(date),
        }
        summary.project = project;
        rows.push(summary);
    }
    Ok(rows)
}

pub fn summarize_by_day(entries: &[LoadedEntry]) -> Result<Vec<UsageSummary>> {
    summarize_by_key_target(
        entries,
        |entry| DayLabel::from(entry.date),
        |day| (day.day().to_string(), None),
        KeyTarget::Date,
    )
}

#[derive(Default)]
struct UsageAccumulator {
    counts: TokenCounts,
    cost: f64,
    credits: Option<f64>,
    message_count: Option<u64>,
    models: Vec<String>,
    breakdowns: Vec<ModelBreakdown>,
    breakdown_indexes: FxHashMap<String, usize>,
}

impl UsageAccumulator {
    fn add_entry(&mut self, entry: &LoadedEntry) {
        let usage = entry.usage;
        self.counts.add_usage(usage);
        self.counts.extra_total_tokens = self
            .counts
            .extra_total_tokens
            .saturating_add(entry.extra_total_tokens);
        self.cost += entry.cost;
        if let Some(credits) = entry.credits {
            *self.credits.get_or_insert(0.0) += credits;
        }
        if let Some(message_count) = entry.message_count {
            *self.message_count.get_or_insert(0) += message_count;
        }
        if let Some(model) = &entry.model {
            let model = crate::model_aliases::resolve_model_name(model);
            let index = if let Some(index) = self.breakdown_indexes.get(model.as_ref()) {
                *index
            } else {
                let index = self.breakdowns.len();
                let owned = model.into_owned();
                self.breakdown_indexes.insert(owned.clone(), index);
                self.models.push(owned.clone());
                self.breakdowns.push(ModelBreakdown {
                    model_name: owned,
                    ..ModelBreakdown::default()
                });
                index
            };
            let breakdown = &mut self.breakdowns[index];
            breakdown.add_usage(&usage);
            breakdown.extra_total_tokens = breakdown
                .extra_total_tokens
                .saturating_add(entry.extra_total_tokens);
            breakdown.cost += entry.cost;
            if entry.missing_pricing_model.is_some() {
                breakdown.missing_pricing = true;
            }
            breakdown.estimated_pricing |= entry.estimated_pricing;
            breakdown.add_stored_cost(entry.stored_cost);
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
            extra_total_tokens: self.counts.extra_total_tokens,
            total_cost: self.cost,
            credits: self.credits,
            message_count: self.message_count,
            models_used: self.models,
            model_breakdowns: self.breakdowns,
            project: None,
            versions: None,
        }
    }
}

#[derive(Default)]
pub struct SessionAccumulator {
    usage: UsageAccumulator,
    latest: Option<(TimestampMs, Arc<str>, Arc<str>)>,
    earliest: Option<TimestampMs>,
    versions: BTreeSet<String>,
}

impl SessionAccumulator {
    pub fn add_entry(&mut self, entry: &LoadedEntry) {
        self.usage.add_entry(entry);
        if self
            .latest
            .as_ref()
            .is_none_or(|(timestamp, _, _)| entry.timestamp > *timestamp)
        {
            self.latest = Some((
                entry.timestamp,
                Arc::clone(&entry.session_id),
                Arc::clone(&entry.project_path),
            ));
        }
        if self
            .earliest
            .is_none_or(|earliest| entry.timestamp < earliest)
        {
            self.earliest = Some(entry.timestamp);
        }
        if let Some(version) = &entry.version {
            self.versions.insert(version.clone());
        }
    }

    pub fn into_summary(self) -> Result<UsageSummary> {
        let Some((timestamp, session_id, project_path)) = self.latest else {
            return Err(cli_error("empty session group"));
        };
        let mut summary = self.usage.into_summary();
        summary.session_id = Some(session_id.to_string());
        summary.project_path = Some(project_path.to_string());
        summary.last_activity = Some(format_rfc3339_millis(timestamp));
        summary.first_activity = self.earliest.map(format_rfc3339_millis);
        summary.versions = Some(self.versions.into_iter().collect());
        Ok(summary)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Bucket {
    Monthly,
    Weekly(WeekDay),
}

pub fn summarize_bucket(rows: &[UsageSummary], bucket: Bucket) -> Vec<UsageSummary> {
    let mut groups: BTreeMap<String, Vec<&UsageSummary>> = BTreeMap::new();
    for row in rows {
        let Some(date) = row.date.as_deref() else {
            continue;
        };
        let key = match bucket {
            Bucket::Monthly => date.get(..7).unwrap_or(date).to_string(),
            Bucket::Weekly(start) => week_start(date, start).unwrap_or_else(|| date.to_string()),
        };
        groups.entry(key).or_default().push(row);
    }

    groups
        .into_iter()
        .map(|(key, rows)| {
            let mut summary = aggregate_summaries(&rows);
            match bucket {
                Bucket::Monthly => summary.month = Some(key),
                Bucket::Weekly(_) => summary.week = Some(key),
            }
            summary
        })
        .collect()
}

fn aggregate_summaries(rows: &[&UsageSummary]) -> UsageSummary {
    let mut summary = UsageSummary {
        date: None,
        month: None,
        week: None,
        session_id: None,
        project_path: None,
        last_activity: None,
        first_activity: None,
        input_tokens: 0,
        output_tokens: 0,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        extra_total_tokens: 0,
        total_cost: 0.0,
        credits: None,
        message_count: None,
        models_used: Vec::new(),
        model_breakdowns: Vec::new(),
        project: None,
        versions: None,
    };
    let mut seen_models = FxHashSet::default();
    let mut breakdown_indexes = FxHashMap::<String, usize>::default();

    for row in rows {
        summary.input_tokens = summary.input_tokens.saturating_add(row.input_tokens);
        summary.output_tokens = summary.output_tokens.saturating_add(row.output_tokens);
        summary.cache_creation_tokens = summary
            .cache_creation_tokens
            .saturating_add(row.cache_creation_tokens);
        summary.cache_read_tokens = summary
            .cache_read_tokens
            .saturating_add(row.cache_read_tokens);
        summary.extra_total_tokens = summary
            .extra_total_tokens
            .saturating_add(row.extra_total_tokens);
        summary.total_cost += row.total_cost;
        if let Some(credits) = row.credits {
            *summary.credits.get_or_insert(0.0) += credits;
        }
        if let Some(message_count) = row.message_count {
            *summary.message_count.get_or_insert(0) += message_count;
        }
        for model in &row.models_used {
            if seen_models.insert(model.clone()) {
                summary.models_used.push(model.clone());
            }
        }
        for item in &row.model_breakdowns {
            let index = if let Some(index) = breakdown_indexes.get(item.model_name.as_str()) {
                *index
            } else {
                let index = summary.model_breakdowns.len();
                breakdown_indexes.insert(item.model_name.clone(), index);
                summary.model_breakdowns.push(ModelBreakdown {
                    model_name: item.model_name.clone(),
                    ..ModelBreakdown::default()
                });
                index
            };
            summary.model_breakdowns[index].merge(item);
        }
    }
    ModelBreakdown::sort_by_cost_desc(&mut summary.model_breakdowns);
    summary
}

pub fn sort_summaries(
    rows: &mut [UsageSummary],
    order: &SortOrder,
    date_fn: &dyn Fn(&UsageSummary) -> &str,
) {
    rows.sort_by(|a, b| match order {
        SortOrder::Asc => date_fn(a).cmp(date_fn(b)),
        SortOrder::Desc => date_fn(b).cmp(date_fn(a)),
    });
}

pub fn week_start(date: &str, start: WeekDay) -> Option<String> {
    let date = ccusage_cli::IsoDate::parse(date)?;
    Some(date.week_start(start).to_string())
}

#[cfg(test)]
mod tests;
