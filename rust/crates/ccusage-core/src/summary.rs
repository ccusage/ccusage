use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use crate::{
    LoadedEntry, ModelBreakdown, Result, TimestampMs, TokenCounts, UsageSummary,
    cli::{SharedArgs, SortOrder, WeekDay},
    cli_error,
    fast::{FxHashMap, FxHashSet},
    format_naive_date, format_rfc3339_millis, parse_iso_date,
};

pub fn summarize_by_key<F, M>(
    entries: &[LoadedEntry],
    key_fn: F,
    meta_fn: M,
) -> Result<Vec<UsageSummary>>
where
    F: Fn(&LoadedEntry) -> String,
    M: Fn(&str) -> (String, Option<String>),
{
    let mut groups: BTreeMap<String, UsageAccumulator> = BTreeMap::new();
    for entry in entries {
        groups.entry(key_fn(entry)).or_default().add_entry(entry);
    }

    let mut rows = Vec::with_capacity(groups.len());
    for (key, group) in groups {
        let (date, project) = meta_fn(&key);
        let mut summary = group.into_summary();
        summary.date = Some(date);
        summary.project = project;
        rows.push(summary);
    }
    Ok(rows)
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
        let usage = entry.data.message.usage;
        self.counts.add_usage(usage);
        self.counts.extra_total_tokens += entry.extra_total_tokens;
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
            breakdown.input_tokens += usage.input_tokens;
            breakdown.output_tokens += usage.output_tokens;
            breakdown.cache_creation_tokens += usage.cache_creation_token_count();
            breakdown.cache_read_tokens += usage.cache_read_input_tokens;
            breakdown.extra_total_tokens += entry.extra_total_tokens;
            breakdown.cost += entry.cost;
            if entry.missing_pricing_model.is_some() {
                breakdown.missing_pricing = true;
            }
        }
    }

    fn into_summary(mut self) -> UsageSummary {
        self.breakdowns.sort_by(|a, b| b.cost.total_cmp(&a.cost));
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
        if let Some(version) = &entry.data.version {
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

#[derive(Clone, Copy)]
pub enum BucketKind {
    Monthly,
    Weekly,
}

pub fn summarize_summaries_by_bucket(
    rows: &[UsageSummary],
    kind: BucketKind,
    start: WeekDay,
) -> Vec<UsageSummary> {
    let mut groups: BTreeMap<String, Vec<&UsageSummary>> = BTreeMap::new();
    for row in rows {
        let Some(date) = row.date.as_deref() else {
            continue;
        };
        let bucket = match kind {
            BucketKind::Monthly => date.get(..7).unwrap_or(date).to_string(),
            BucketKind::Weekly => week_start(date, start).unwrap_or_else(|| date.to_string()),
        };
        groups.entry(bucket).or_default().push(row);
    }

    groups
        .into_iter()
        .map(|(bucket, rows)| {
            let mut summary = aggregate_summaries(&rows);
            match kind {
                BucketKind::Monthly => summary.month = Some(bucket),
                BucketKind::Weekly => summary.week = Some(bucket),
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
        summary.input_tokens += row.input_tokens;
        summary.output_tokens += row.output_tokens;
        summary.cache_creation_tokens += row.cache_creation_tokens;
        summary.cache_read_tokens += row.cache_read_tokens;
        summary.extra_total_tokens += row.extra_total_tokens;
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
            let breakdown = &mut summary.model_breakdowns[index];
            breakdown.input_tokens += item.input_tokens;
            breakdown.output_tokens += item.output_tokens;
            breakdown.cache_creation_tokens += item.cache_creation_tokens;
            breakdown.cache_read_tokens += item.cache_read_tokens;
            breakdown.extra_total_tokens += item.extra_total_tokens;
            breakdown.cost += item.cost;
            breakdown.missing_pricing |= item.missing_pricing;
        }
    }
    summary
        .model_breakdowns
        .sort_by(|a, b| b.cost.total_cmp(&a.cost));
    summary
}

pub fn filter_and_sort_summaries<F>(rows: &mut Vec<UsageSummary>, shared: &SharedArgs, date_fn: F)
where
    F: Fn(&UsageSummary) -> &str,
{
    if shared.since.is_some() || shared.until.is_some() {
        rows.retain(|row| {
            crate::date_within_range(
                date_fn(row),
                shared.since.as_deref(),
                shared.until.as_deref(),
            )
        });
    }
    sort_summaries(rows, &shared.order, date_fn);
}

pub fn sort_summaries<F>(rows: &mut [UsageSummary], order: &SortOrder, date_fn: F)
where
    F: Fn(&UsageSummary) -> &str,
{
    rows.sort_by(|a, b| match order {
        SortOrder::Asc => date_fn(a).cmp(date_fn(b)),
        SortOrder::Desc => date_fn(b).cmp(date_fn(a)),
    });
}

pub fn week_start(date: &str, start: WeekDay) -> Option<String> {
    let date = parse_iso_date(date)?;
    let start_num = match start {
        WeekDay::Sunday => 0,
        WeekDay::Monday => 1,
        WeekDay::Tuesday => 2,
        WeekDay::Wednesday => 3,
        WeekDay::Thursday => 4,
        WeekDay::Friday => 5,
        WeekDay::Saturday => 6,
    };
    let day = date.weekday_from_sunday() as i64;
    let shift = (day - start_num + 7) % 7;
    Some(format_naive_date(date.checked_add_days(-shift)?))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{
        ModelBreakdown, TokenUsageRaw, UsageEntry, UsageMessage,
        cli::{SharedArgs, SortOrder, normalize_date_bound},
        format_rfc3339_millis,
    };

    #[test]
    fn snapshots_summarize_by_key_aggregates_counts_costs_and_breakdowns() {
        let entries = vec![
            loaded_entry(LoadedEntryFixture {
                date: "2026-01-02",
                timestamp: 1_767_316_800_000,
                session_id: "session-a",
                project_path: "/workspace/api",
                model: Some("gpt-5.2-codex"),
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_tokens: 10,
                cache_read_tokens: 5,
                extra_total_tokens: 7,
                cost: 0.125,
                credits: Some(1.0),
                message_count: Some(2),
                version: Some("1.0.0"),
                missing_pricing_model: None,
            }),
            loaded_entry(LoadedEntryFixture {
                date: "2026-01-02",
                timestamp: 1_767_320_400_000,
                session_id: "session-b",
                project_path: "/workspace/api",
                model: Some("claude-sonnet-4-20250514"),
                input_tokens: 200,
                output_tokens: 80,
                cache_creation_tokens: 20,
                cache_read_tokens: 8,
                extra_total_tokens: 0,
                cost: 0.25,
                credits: Some(0.5),
                message_count: Some(3),
                version: Some("1.0.1"),
                missing_pricing_model: None,
            }),
            loaded_entry(LoadedEntryFixture {
                date: "2026-01-03",
                timestamp: 1_767_402_000_000,
                session_id: "session-c",
                project_path: "/workspace/web",
                model: None,
                input_tokens: 10,
                output_tokens: 0,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                extra_total_tokens: 5,
                cost: 0.0,
                credits: None,
                message_count: None,
                version: None,
                missing_pricing_model: None,
            }),
        ];

        let rows = summarize_by_key(
            &entries,
            |entry| format!("{}|{}", entry.date, entry.project_path),
            |key| {
                let (date, project) = key.split_once('|').unwrap();
                (date.to_string(), Some(project.to_string()))
            },
        )
        .unwrap();

        insta::assert_json_snapshot!(rows);
    }

    #[test]
    fn snapshots_session_accumulator_latest_metadata_versions_and_timezone() {
        let mut accumulator = SessionAccumulator::default();
        accumulator.add_entry(&loaded_entry(LoadedEntryFixture {
            date: "2026-01-02",
            timestamp: 1_767_316_800_000,
            session_id: "older-session",
            project_path: "/workspace/old",
            model: Some("gpt-5.2-codex"),
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_tokens: 10,
            cache_read_tokens: 5,
            extra_total_tokens: 0,
            cost: 0.125,
            credits: None,
            message_count: Some(1),
            version: Some("1.0.0"),
            missing_pricing_model: None,
        }));
        accumulator.add_entry(&loaded_entry(LoadedEntryFixture {
            date: "2026-01-03",
            timestamp: 1_767_402_000_000,
            session_id: "latest-session",
            project_path: "/workspace/new",
            model: Some("claude-sonnet-4-20250514"),
            input_tokens: 20,
            output_tokens: 10,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            extra_total_tokens: 3,
            cost: 0.25,
            credits: None,
            message_count: Some(4),
            version: Some("1.1.0"),
            missing_pricing_model: None,
        }));

        let row = accumulator.into_summary().unwrap();

        insta::assert_json_snapshot!(row);
    }

    #[test]
    fn snapshots_bucket_aggregation_for_week_boundaries_invalid_dates_and_model_merging() {
        let rows = vec![
            summary_row(SummaryFixture {
                date: Some("2025-12-31"),
                model: "gpt-5.2-codex",
                cost: 0.125,
                input_tokens: 100,
            }),
            summary_row(SummaryFixture {
                date: Some("2026-01-01"),
                model: "claude-sonnet-4-20250514",
                cost: 0.25,
                input_tokens: 200,
            }),
            summary_row(SummaryFixture {
                date: Some("2026-01-05"),
                model: "gpt-5.2-codex",
                cost: 0.5,
                input_tokens: 300,
            }),
            summary_row(SummaryFixture {
                date: Some("not-a-date"),
                model: "unknown-model",
                cost: 0.0,
                input_tokens: 1,
            }),
            summary_row(SummaryFixture {
                date: None,
                model: "ignored-no-date",
                cost: 9.99,
                input_tokens: 999,
            }),
        ];

        let weekly = summarize_summaries_by_bucket(&rows, BucketKind::Weekly, WeekDay::Monday);
        let monthly = summarize_summaries_by_bucket(&rows, BucketKind::Monthly, WeekDay::Sunday);

        insta::assert_json_snapshot!(serde_json::json!({
            "weekly": weekly,
            "monthly": monthly,
        }));
    }

    #[test]
    fn snapshots_filter_and_sort_summaries_since_until_inclusive() {
        let mut rows = vec![
            summary_row(SummaryFixture {
                date: Some("2026-01-01"),
                model: "before",
                cost: 0.125,
                input_tokens: 1,
            }),
            summary_row(SummaryFixture {
                date: Some("2026-01-02"),
                model: "start-boundary",
                cost: 0.25,
                input_tokens: 2,
            }),
            summary_row(SummaryFixture {
                date: Some("2026-01-10"),
                model: "end-boundary",
                cost: 0.5,
                input_tokens: 10,
            }),
            summary_row(SummaryFixture {
                date: Some("2026-01-11"),
                model: "after",
                cost: 1.0,
                input_tokens: 11,
            }),
        ];
        let shared = SharedArgs {
            since: Some("20260102".to_string()),
            until: Some("20260110".to_string()),
            order: SortOrder::Desc,
            ..SharedArgs::default()
        };

        filter_and_sort_summaries(&mut rows, &shared, |row| row.date.as_deref().unwrap_or(""));

        insta::assert_json_snapshot!(rows);
    }

    #[test]
    fn filter_and_sort_summaries_treats_dashed_and_compact_bounds_equally() {
        fn filtered_models(since: &str) -> Vec<String> {
            let mut rows = vec![
                summary_row(SummaryFixture {
                    date: Some("2026-01-24"),
                    model: "before",
                    cost: 0.125,
                    input_tokens: 1,
                }),
                summary_row(SummaryFixture {
                    date: Some("2026-04-22"),
                    model: "on-boundary",
                    cost: 0.25,
                    input_tokens: 2,
                }),
                summary_row(SummaryFixture {
                    date: Some("2026-04-23"),
                    model: "after",
                    cost: 0.5,
                    input_tokens: 3,
                }),
            ];
            let shared = SharedArgs {
                since: normalize_date_bound(since),
                order: SortOrder::Asc,
                ..SharedArgs::default()
            };

            filter_and_sort_summaries(&mut rows, &shared, |row| row.date.as_deref().unwrap_or(""));

            rows.into_iter()
                .map(|row| row.models_used.into_iter().next().unwrap())
                .collect()
        }

        let dashed = filtered_models("2026-04-22");
        let compact = filtered_models("20260422");

        assert_eq!(dashed, compact);
        assert_eq!(dashed, vec!["on-boundary", "after"]);
    }

    #[test]
    fn tracks_missing_pricing_in_model_breakdowns() {
        let mut accumulator = SessionAccumulator::default();
        accumulator.add_entry(&loaded_entry(LoadedEntryFixture {
            date: "2026-01-02",
            timestamp: 1_767_316_800_000,
            session_id: "session-a",
            project_path: "/workspace/project",
            model: Some("claude-opus-4-9"),
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_tokens: 10,
            cache_read_tokens: 5,
            extra_total_tokens: 0,
            cost: 0.0,
            credits: None,
            message_count: Some(1),
            version: Some("1.0.0"),
            missing_pricing_model: Some("claude-opus-4-9"),
        }));

        let row = accumulator.into_summary().unwrap();

        assert_eq!(row.model_breakdowns[0].model_name, "claude-opus-4-9");
        assert!(row.model_breakdowns[0].missing_pricing);
    }

    #[test]
    fn applies_model_aliases_to_model_breakdowns() {
        let _aliases = crate::model_aliases::set_model_aliases_for_tests([
            ("private-alpha", "gpt-5.5"),
            ("another-private-alpha", "gpt-5.5"),
        ]);
        let mut accumulator = SessionAccumulator::default();
        accumulator.add_entry(&loaded_entry(LoadedEntryFixture {
            date: "2026-01-02",
            timestamp: 1_767_316_800_000,
            session_id: "session-a",
            project_path: "/workspace/project",
            model: Some("private-alpha"),
            input_tokens: 20,
            output_tokens: 5,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            extra_total_tokens: 0,
            cost: 0.02,
            credits: None,
            message_count: Some(1),
            version: Some("1.0.0"),
            missing_pricing_model: None,
        }));
        accumulator.add_entry(&loaded_entry(LoadedEntryFixture {
            date: "2026-01-02",
            timestamp: 1_767_316_801_000,
            session_id: "session-a",
            project_path: "/workspace/project",
            model: Some("another-private-alpha"),
            input_tokens: 30,
            output_tokens: 7,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            extra_total_tokens: 0,
            cost: 0.03,
            credits: None,
            message_count: Some(1),
            version: Some("1.0.0"),
            missing_pricing_model: None,
        }));

        let row = accumulator.into_summary().unwrap();

        assert_eq!(row.models_used, vec!["gpt-5.5"]);
        assert_eq!(row.model_breakdowns.len(), 1);
        assert_eq!(row.model_breakdowns[0].model_name, "gpt-5.5");
        assert_eq!(row.model_breakdowns[0].input_tokens, 50);
        assert_eq!(row.model_breakdowns[0].output_tokens, 12);
        assert_eq!(row.model_breakdowns[0].cost, 0.05);
    }

    struct LoadedEntryFixture {
        date: &'static str,
        timestamp: i64,
        session_id: &'static str,
        project_path: &'static str,
        model: Option<&'static str>,
        input_tokens: u64,
        output_tokens: u64,
        cache_creation_tokens: u64,
        cache_read_tokens: u64,
        extra_total_tokens: u64,
        cost: f64,
        credits: Option<f64>,
        message_count: Option<u64>,
        version: Option<&'static str>,
        missing_pricing_model: Option<&'static str>,
    }

    fn loaded_entry(fixture: LoadedEntryFixture) -> LoadedEntry {
        let usage = TokenUsageRaw {
            input_tokens: fixture.input_tokens,
            output_tokens: fixture.output_tokens,
            cache_creation_input_tokens: fixture.cache_creation_tokens,
            cache_read_input_tokens: fixture.cache_read_tokens,
            speed: None,
            cache_creation: None,
        };
        let timestamp = TimestampMs::from_millis(fixture.timestamp);
        LoadedEntry {
            data: UsageEntry {
                session_id: Some(fixture.session_id.to_string()),
                timestamp: format_rfc3339_millis(timestamp),
                version: fixture.version.map(str::to_string),
                message: UsageMessage {
                    usage,
                    model: fixture.model.map(str::to_string),
                    id: Some(format!("msg-{}", fixture.timestamp)),
                },
                cost_usd: None,
                request_id: None,
                is_api_error_message: None,
                is_sidechain: None,
            },
            timestamp,
            date: fixture.date.to_string(),
            project: Arc::from(fixture.project_path),
            session_id: Arc::from(fixture.session_id),
            project_path: Arc::from(fixture.project_path),
            cost: fixture.cost,
            extra_total_tokens: fixture.extra_total_tokens,
            credits: fixture.credits,
            message_count: fixture.message_count,
            model: fixture.model.map(str::to_string),
            usage_limit_reset_time: None,
            missing_pricing_model: fixture.missing_pricing_model.map(str::to_string),
        }
    }

    struct SummaryFixture {
        date: Option<&'static str>,
        model: &'static str,
        cost: f64,
        input_tokens: u64,
    }

    fn summary_row(fixture: SummaryFixture) -> UsageSummary {
        UsageSummary {
            date: fixture.date.map(str::to_string),
            month: None,
            week: None,
            session_id: None,
            project_path: None,
            last_activity: None,
            first_activity: None,
            input_tokens: fixture.input_tokens,
            output_tokens: 10,
            cache_creation_tokens: 1,
            cache_read_tokens: 2,
            extra_total_tokens: 3,
            total_cost: fixture.cost,
            credits: Some(0.5),
            message_count: Some(1),
            models_used: vec![fixture.model.to_string()],
            model_breakdowns: vec![ModelBreakdown {
                model_name: fixture.model.to_string(),
                input_tokens: fixture.input_tokens,
                output_tokens: 10,
                cache_creation_tokens: 1,
                cache_read_tokens: 2,
                extra_total_tokens: 3,
                cost: fixture.cost,
                missing_pricing: false,
            }],
            project: None,
            versions: None,
        }
    }
}
