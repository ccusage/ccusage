use ccusage_core::{
    Diagnostics, LoadedEntry, ReportWindow, Result, UsageSummary,
    cli::{AgentCommandArgs, AgentReportKind},
    parse_ts_timestamp, print_json_or_jq,
    progress::UsageLoadAgent,
    sort_summaries, wants_json,
};

use crate::{
    rows::summary_rows,
    source::{
        Detection, EntrySource, LoadContext, LoadRequest, LoadedRows, ProgressOwner, ReportPricing,
        SourceOptions, WindowMode,
    },
};

#[cfg(any(test, feature = "testing"))]
pub mod parity;

pub(crate) fn run_focused(source: &dyn EntrySource, args: AgentCommandArgs) -> Result<()> {
    let rows = focused_rows(source, &args)?;
    let shared = &args.shared;
    if wants_json(shared) {
        return print_json_or_jq(
            source.report_json(&rows, args.kind),
            shared.jq.as_deref(),
            shared.no_cost,
        );
    }
    source.print_table(&rows, args.kind, shared)
}

fn focused_rows(source: &dyn EntrySource, args: &AgentCommandArgs) -> Result<Vec<UsageSummary>> {
    let descriptor = source.descriptor();
    let shared = &args.shared;
    let pricing = ReportPricing::for_load(shared, None);
    let window = ReportWindow::from_shared(shared);
    let diagnostics = Diagnostics::new(descriptor.id);
    let cx = LoadContext {
        shared,
        kind: args.kind,
        window: &window,
        pricing,
        options: SourceOptions::from_command(args),
        diagnostics: &diagnostics,
    };
    let entries = load(source, &cx, ProgressOwner::Source)?;
    diagnostics.emit(shared.debug);
    let mut rows = summarize_in_window(descriptor.window, entries, &cx, |entries, kind| {
        source.summarize(
            entries,
            crate::source::SummaryKind::for_report(kind, args.start_of_week),
        )
    })?;
    sort_summaries(&mut rows, &shared.order.unwrap_or_default(), &|row| {
        source.sort_key(row)
    });
    Ok(rows)
}

pub(crate) fn load_rows(source: &dyn EntrySource, request: &LoadRequest<'_>) -> Result<LoadedRows> {
    let descriptor = source.descriptor();
    let shared = request.shared;
    let pricing = ReportPricing::for_load(shared, request.pricing);
    let window = ReportWindow::from_shared(shared);
    let diagnostics = Diagnostics::new(descriptor.id);
    let cx = LoadContext {
        shared,
        kind: request.kind.report_kind(),
        window: &window,
        pricing,
        options: request.options,
        diagnostics: &diagnostics,
    };
    let entries = load(source, &cx, request.progress)?;
    let detected = detect(descriptor.detection, &entries);
    let summaries = summarize_in_window(descriptor.window, entries, &cx, |entries, _kind| {
        source.summarize(entries, crate::source::SummaryKind::for_load(request.kind))
    })?;
    Ok(LoadedRows {
        rows: summary_rows(descriptor.id, summaries, descriptor.unified_project_path),
        detected,
        diagnostics,
    })
}

fn load(
    source: &dyn EntrySource,
    cx: &LoadContext<'_>,
    progress: ProgressOwner,
) -> Result<Vec<LoadedEntry>> {
    progress.run(
        UsageLoadAgent(source.descriptor().label),
        cx.shared.json,
        || source.load_entries(cx),
    )
}

fn detect(detection: Detection, entries: &[LoadedEntry]) -> bool {
    match detection {
        Detection::NonEmpty => !entries.is_empty(),
        Detection::NonEmptyOrHasData(has_data) => !entries.is_empty() || has_data(),
    }
}

fn summarize_in_window(
    mode: WindowMode,
    mut entries: Vec<LoadedEntry>,
    cx: &LoadContext<'_>,
    summarize: impl FnOnce(&[LoadedEntry], AgentReportKind) -> Result<Vec<UsageSummary>>,
) -> Result<Vec<UsageSummary>> {
    if mode == WindowMode::SessionLastActivity && cx.kind == AgentReportKind::Session {
        let mut rows = summarize(&entries, cx.kind)?;
        retain_sessions_last_active_in(&mut rows, cx.window);
        return Ok(rows);
    }
    retain_entries_in(&mut entries, cx.window);
    summarize(&entries, cx.kind)
}

pub(crate) fn retain_entries_in(entries: &mut Vec<LoadedEntry>, window: &ReportWindow) {
    if window.is_unbounded() {
        return;
    }
    entries.retain(|entry| window.contains_entry_date(entry.date));
}

fn retain_sessions_last_active_in(rows: &mut Vec<UsageSummary>, window: &ReportWindow) {
    if window.is_unbounded() {
        return;
    }
    rows.retain(|row| {
        row.last_activity
            .as_deref()
            .and_then(parse_ts_timestamp)
            .is_some_and(|timestamp| window.contains_timestamp(timestamp))
    });
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::atomic::Ordering};

    use ccusage_core::{
        PricingMap, ReportWindow, UsageSummary,
        cli::{AgentCommandArgs, AgentReportKind, SharedArgs, SortOrder},
    };

    use super::{focused_rows, load_rows, retain_sessions_last_active_in};
    use crate::{
        source::{
            Detection, EntryDescriptor, LoadRequest, ProgressOwner, SourceOptions, UsageSource,
            WindowMode,
        },
        test_source::{Fake, descriptor, entries, shared},
    };

    fn command(kind: AgentReportKind, shared: SharedArgs) -> AgentCommandArgs {
        AgentCommandArgs {
            options: Default::default(),
            shared,
            kind,
            session_id: None,
            sections: None,
            by_agent: false,

            include_claude_agent: false,
            start_of_week: ccusage_core::cli::WeekDay::Sunday,
            sources: BTreeMap::new(),
        }
    }

    fn request(kind: AgentReportKind, shared: &SharedArgs) -> LoadRequest<'_> {
        LoadRequest {
            kind: crate::source::LoadKind::for_report(kind),
            shared,
            options: SourceOptions::default(),
            pricing: None,
            progress: ProgressOwner::Caller,
        }
    }

    fn focused_tokens(
        source: &Fake,
        kind: AgentReportKind,
        shared: &SharedArgs,
    ) -> Vec<(String, u64)> {
        focused_rows(source, &command(kind, shared.clone()))
            .unwrap()
            .into_iter()
            .map(|row| {
                (
                    ccusage_core::summary_period(&row).to_string(),
                    row.input_tokens,
                )
            })
            .collect()
    }

    fn unified_tokens(
        source: &Fake,
        kind: AgentReportKind,
        shared: &SharedArgs,
    ) -> Vec<(String, u64)> {
        load_rows(source, &request(kind, shared))
            .unwrap()
            .rows
            .into_iter()
            .map(|row| (row.usage.period, row.usage.input_tokens))
            .collect()
    }

    #[test]
    fn both_reports_drop_entries_outside_the_window() {
        let source = Fake::new(descriptor(), entries());
        let shared = shared(Some("20260102"), Some("20260102"));
        let expected = [("2026-01-02".to_string(), 100)];

        assert_eq!(
            focused_tokens(&source, AgentReportKind::Daily, &shared),
            expected
        );
        assert_eq!(
            unified_tokens(&source, AgentReportKind::Daily, &shared),
            expected
        );
    }

    #[test]
    fn entries_mode_cuts_a_session_at_the_window_edge() {
        let source = Fake::new(descriptor(), entries());
        let shared = shared(Some("20260102"), Some("20260102"));
        let expected = [("long".to_string(), 100)];

        assert_eq!(
            focused_tokens(&source, AgentReportKind::Session, &shared),
            expected
        );
        assert_eq!(
            unified_tokens(&source, AgentReportKind::Session, &shared),
            expected
        );
    }

    #[test]
    fn last_activity_mode_keeps_whole_sessions_that_end_in_the_window() {
        let source = Fake::new(
            EntryDescriptor {
                window: WindowMode::SessionLastActivity,
                ..descriptor()
            },
            entries(),
        );
        let ends_inside = shared(Some("20260102"), None);
        let ends_after = shared(None, Some("20260102"));

        for (shared, expected) in [
            (&ends_inside, vec![("long".to_string(), 1110)]),
            (&ends_after, vec![("early".to_string(), 1)]),
        ] {
            assert_eq!(
                focused_tokens(&source, AgentReportKind::Session, shared),
                expected
            );
            assert_eq!(
                unified_tokens(&source, AgentReportKind::Session, shared),
                expected
            );
        }
        assert_eq!(
            unified_tokens(&source, AgentReportKind::Daily, &ends_inside),
            [
                ("2026-01-02".to_string(), 100),
                ("2026-01-03".to_string(), 1000)
            ],
            "other kinds still filter entries"
        );
    }

    #[test]
    fn focused_rows_follow_the_requested_order() {
        let source = Fake::new(descriptor(), entries());
        let shared = SharedArgs {
            order: Some(SortOrder::Desc),
            ..shared(None, None)
        };

        let periods = focused_tokens(&source, AgentReportKind::Daily, &shared)
            .into_iter()
            .map(|(period, _)| period)
            .collect::<Vec<_>>();

        assert_eq!(periods, ["2026-01-03", "2026-01-02", "2026-01-01"]);
    }

    #[test]
    fn detection_ignores_the_window() {
        let source = Fake::new(descriptor(), entries());
        let shared = shared(Some("20300101"), None);

        let rows = load_rows(&source, &request(AgentReportKind::Daily, &shared)).unwrap();

        assert!(rows.rows.is_empty());
        assert!(rows.detected);
    }

    #[test]
    fn detection_can_fall_back_to_a_probe_of_the_source() {
        let shared = shared(None, None);
        let request = request(AgentReportKind::Daily, &shared);
        let absent = Fake::new(descriptor(), Vec::new());
        let narrowed_by_its_loader = Fake::new(
            EntryDescriptor {
                detection: Detection::NonEmptyOrHasData(|| true),
                ..descriptor()
            },
            Vec::new(),
        );

        assert!(!load_rows(&absent, &request).unwrap().detected);
        assert!(!absent.has_data());
        assert!(
            load_rows(&narrowed_by_its_loader, &request)
                .unwrap()
                .detected
        );
        assert!(narrowed_by_its_loader.has_data());
    }

    #[test]
    fn pricing_reaches_focused_and_unified_entry_loaders() {
        let shared = shared(None, None);
        let priced_by_pipeline = Fake::new(descriptor(), entries());
        let pricing = PricingMap::default();

        focused_rows(
            &priced_by_pipeline,
            &command(AgentReportKind::Daily, shared.clone()),
        )
        .unwrap();
        assert!(priced_by_pipeline.saw_pricing.load(Ordering::Relaxed));

        priced_by_pipeline
            .saw_pricing
            .store(false, Ordering::Relaxed);
        load_rows(
            &priced_by_pipeline,
            &LoadRequest {
                pricing: Some(&pricing),
                ..request(AgentReportKind::Daily, &shared)
            },
        )
        .unwrap();
        assert!(priced_by_pipeline.saw_pricing.load(Ordering::Relaxed));
    }

    #[test]
    fn load_rows_carry_what_the_load_skipped() {
        let source = Fake {
            skips_a_unit: true,
            ..Fake::new(descriptor(), entries())
        };
        let shared = SharedArgs {
            debug: true,
            ..shared(None, None)
        };

        let rows = load_rows(&source, &request(AgentReportKind::Daily, &shared)).unwrap();

        assert_eq!(rows.rows.len(), 3, "the rest of the source still reports");
        let warnings = rows.diagnostics.warnings(&shared);
        assert!(
            warnings
                .contains(&"fake: unreadable: /data/broken.json: permission denied".to_string()),
            "{warnings:?}"
        );
    }

    #[test]
    fn unified_session_rows_carry_the_project_path_on_request() {
        let shared = shared(None, None);
        let request = request(AgentReportKind::Session, &shared);
        let with_path = Fake::new(
            EntryDescriptor {
                unified_project_path: true,
                ..descriptor()
            },
            entries(),
        );
        let without_path = Fake::new(descriptor(), entries());

        let project_path = |source: &Fake| {
            load_rows(source, &request).unwrap().rows[0]
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.project_path.clone())
        };

        assert_eq!(project_path(&with_path), Some("project".to_string()));
        assert_eq!(project_path(&without_path), None);
    }

    #[test]
    fn an_entry_source_is_a_usage_source() {
        let source = Fake::new(descriptor(), entries());
        let source: &dyn UsageSource = &source;

        assert_eq!(source.id(), "fake");
        assert_eq!(source.label(), "Fake");
        assert_eq!(
            source
                .load_rows(&request(AgentReportKind::Daily, &shared(None, None)))
                .unwrap()
                .rows
                .len(),
            3
        );
    }

    fn session_summary(session_id: &str, last_activity: &str) -> UsageSummary {
        UsageSummary {
            date: None,
            month: None,
            week: None,
            session_id: Some(session_id.to_string()),
            project_path: None,
            last_activity: Some(last_activity.to_string()),
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
        }
    }

    fn filtered_session_ids(shared: &SharedArgs) -> Vec<String> {
        let mut rows = vec![
            session_summary("before", "2026-02-22T23:59:59.000Z"),
            session_summary("start-of-day", "2026-02-23T00:00:00.000Z"),
            session_summary("end-of-day", "2026-02-23T23:59:59.999Z"),
            session_summary("after", "2026-02-24T00:00:00.000Z"),
        ];
        retain_sessions_last_active_in(&mut rows, &ReportWindow::from_shared(shared));
        rows.into_iter().filter_map(|row| row.session_id).collect()
    }

    #[test]
    fn session_window_bounds_are_inclusive_of_the_whole_day() {
        let shared = SharedArgs {
            since: Some("20260223".parse().unwrap()),
            until: Some("20260223".parse().unwrap()),
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };

        assert_eq!(
            filtered_session_ids(&shared),
            ["start-of-day", "end-of-day"]
        );
    }

    #[test]
    fn session_window_takes_the_day_in_the_report_timezone() {
        let shared = SharedArgs {
            since: Some("20260223".parse().unwrap()),
            until: Some("20260223".parse().unwrap()),
            timezone: Some("Asia/Tokyo".parse().unwrap()),
            ..SharedArgs::default()
        };

        assert_eq!(filtered_session_ids(&shared), ["before", "start-of-day"]);
    }

    #[test]
    fn session_window_with_only_until_keeps_everything_through_that_day() {
        let shared = SharedArgs {
            until: Some("20260223".parse().unwrap()),
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };

        assert_eq!(
            filtered_session_ids(&shared),
            ["before", "start-of-day", "end-of-day"]
        );
    }

    #[test]
    fn session_window_with_only_since_keeps_everything_from_that_day() {
        let shared = SharedArgs {
            since: Some("20260223".parse().unwrap()),
            timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
            ..SharedArgs::default()
        };

        assert_eq!(
            filtered_session_ids(&shared),
            ["start-of-day", "end-of-day", "after"]
        );
    }

    #[test]
    fn session_window_drops_rows_without_a_parseable_last_activity() {
        let missing = UsageSummary {
            last_activity: None,
            ..session_summary("missing", "")
        };
        let unparsable = session_summary("unparsable", "2026-02-23");
        for shared in [
            SharedArgs {
                since: Some("20260101".parse().unwrap()),
                ..SharedArgs::default()
            },
            SharedArgs {
                until: Some("20991231".parse().unwrap()),
                ..SharedArgs::default()
            },
        ] {
            let mut rows = vec![missing.clone(), unparsable.clone()];
            retain_sessions_last_active_in(&mut rows, &ReportWindow::from_shared(&shared));
            assert!(rows.is_empty(), "{:?}/{:?}", shared.since, shared.until);
        }
    }
}
