use ccusage_core::{UsageSummary, cli::SharedArgs, date_within_range, sort_summaries};

pub fn filter_and_sort_summaries<F>(rows: &mut Vec<UsageSummary>, shared: &SharedArgs, date_fn: F)
where
    F: Fn(&UsageSummary) -> &str,
{
    if shared.since.is_some() || shared.until.is_some() {
        rows.retain(|row| {
            date_within_range(
                date_fn(row),
                shared.since.as_deref(),
                shared.until.as_deref(),
            )
        });
    }
    sort_summaries(rows, &shared.order, date_fn);
}

#[cfg(test)]
mod tests {
    use ccusage_core::{
        ModelBreakdown,
        cli::{SortOrder, normalize_date_bound},
    };

    use super::*;

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
                stored_cost: None,
            }],
            project: None,
            versions: None,
        }
    }
}
