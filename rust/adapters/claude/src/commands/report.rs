use std::sync::Arc;

use ccusage_core::{
    DEFAULT_RECENT_DAYS, MILLIS_PER_DAY, ReportWindow, Result, SessionAccumulator, TimestampMs,
    UsageSummary,
    cli::{
        BlocksArgs, ClaudeReport, DailyArgs, SessionArgs, SharedArgs, SortOrder,
        session_cost_ordering,
    },
    fast::FxHashMap,
    print_json_or_jq, print_usage_table, sort_summaries,
    summary::{Bucket, summarize_bucket, summarize_by_day},
    totals_json, wants_json,
};
use serde_json::json;

use crate::{
    blocks::{
        block_json, filter_blocks_by_date, identify_session_blocks, print_active_block_detail,
        print_blocks_table, sort_blocks,
    },
    load_daily_summaries, load_entries, load_entries_since,
};

mod json;

use json::{group_project_output, session_summary_json, summary_json};

pub fn run_report(report: ClaudeReport, project: Option<&str>) -> Result<()> {
    match report {
        ClaudeReport::Daily(args) => run_daily(args, project),
        ClaudeReport::Monthly(shared) => run_bucket(shared, Bucket::Monthly, project),
        ClaudeReport::Weekly(args) => {
            run_bucket(args.shared, Bucket::Weekly(args.start_of_week), project)
        }
        ClaudeReport::Session(args) => run_session(args, project),
        ClaudeReport::Blocks(args) => run_blocks(args, project),
    }
}

fn run_daily(args: DailyArgs, project: Option<&str>) -> Result<()> {
    let shared = args.shared.clone();
    let mut rows = load_daily_summaries(&shared, project, args.instances || project.is_some())?;
    sort_summaries(&mut rows, &shared.order.unwrap_or_default(), &|row| {
        row.date.as_deref().unwrap_or_default()
    });

    if wants_json(&shared) {
        if args.instances && rows.iter().any(|row| row.project.is_some()) {
            let output = json!({
                "projects": group_project_output(&rows),
                "totals": totals_json(&rows),
            });
            print_json_or_jq(output, shared.jq.as_deref(), shared.no_cost)?;
        } else {
            let output = json!({
                "daily": rows.iter().map(summary_json).collect::<Vec<_>>(),
                "totals": totals_json(&rows),
            });
            print_json_or_jq(output, shared.jq.as_deref(), shared.no_cost)?;
        }
        return Ok(());
    }

    print_usage_table(
        "Claude Code Token Usage Report - Daily",
        "Date",
        &rows,
        &shared,
        args.instances,
        args.project_aliases.as_deref(),
    )?;
    Ok(())
}

fn run_bucket(shared: SharedArgs, bucket: Bucket, project: Option<&str>) -> Result<()> {
    let entries = load_entries_since(&shared, project)?
        .into_iter()
        .map(|entry| entry.entry)
        .collect::<Vec<_>>();
    let mut daily = summarize_by_day(&entries)?;
    sort_summaries(&mut daily, &shared.order.unwrap_or_default(), &|row| {
        row.date.as_deref().unwrap_or_default()
    });

    let mut buckets = summarize_bucket(&daily, bucket);
    sort_summaries(
        &mut buckets,
        &shared.order.unwrap_or_default(),
        &|row| match bucket {
            Bucket::Monthly => row.month.as_deref().unwrap_or_default(),
            Bucket::Weekly(_) => row.week.as_deref().unwrap_or_default(),
        },
    );

    if wants_json(&shared) {
        let key = match bucket {
            Bucket::Monthly => "monthly",
            Bucket::Weekly(_) => "weekly",
        };
        let output = json!({
            key: buckets.iter().map(summary_json).collect::<Vec<_>>(),
            "totals": totals_json(&buckets),
        });
        print_json_or_jq(output, shared.jq.as_deref(), shared.no_cost)?;
        return Ok(());
    }

    let (title, col) = match bucket {
        Bucket::Monthly => ("Claude Code Token Usage Report - Monthly", "Month"),
        Bucket::Weekly(_) => ("Claude Code Token Usage Report - Weekly", "Week"),
    };
    print_usage_table(title, col, &buckets, &shared, false, None)?;
    Ok(())
}

fn run_session(args: SessionArgs, project: Option<&str>) -> Result<()> {
    let shared = args.shared.clone();
    if let Some(id) = args.id {
        return run_session_id(&id, &shared, project);
    }

    let mut rows = load_session_rows(&shared, project)?;
    rows.retain(|row| {
        row.input_tokens > 0
            || row.output_tokens > 0
            || row.cache_creation_tokens > 0
            || row.cache_read_tokens > 0
    });
    ccusage_core::sort::stable_by(&mut rows, |a, b| match session_cost_ordering(&shared) {
        SortOrder::Asc => a.total_cost.total_cmp(&b.total_cost),
        SortOrder::Desc => b.total_cost.total_cmp(&a.total_cost),
    });

    if wants_json(&shared) {
        let output = json!({
            "sessions": rows.iter().map(session_summary_json).collect::<Vec<_>>(),
            "totals": totals_json(&rows),
        });
        print_json_or_jq(output, shared.jq.as_deref(), shared.no_cost)?;
        return Ok(());
    }

    print_usage_table(
        "Claude Code Token Usage Report - By Session",
        "Session",
        &rows,
        &shared,
        false,
        None,
    )?;
    Ok(())
}

fn load_session_rows(shared: &SharedArgs, project: Option<&str>) -> Result<Vec<UsageSummary>> {
    let entries = load_entries_since(shared, project)?;
    let mut grouped = Vec::<SessionAccumulator>::new();
    let mut group_indexes = FxHashMap::<(Arc<str>, Arc<str>), usize>::default();
    for entry in &entries {
        let key = (
            Arc::clone(&entry.project_path),
            Arc::clone(&entry.session_id),
        );
        let index = *group_indexes.entry(key).or_insert_with(|| {
            let index = grouped.len();
            grouped.push(SessionAccumulator::default());
            index
        });
        grouped[index].add_entry(entry);
    }

    let mut rows = Vec::with_capacity(grouped.len());
    for group in grouped {
        rows.push(group.into_summary()?);
    }
    Ok(rows)
}

fn run_session_id(id: &str, shared: &SharedArgs, project: Option<&str>) -> Result<()> {
    let entries = load_entries_since(shared, project)?;
    let mut session_entries = entries
        .into_iter()
        .filter(|entry| {
            entry.recorded_session_id.as_deref() == Some(id) || entry.session_id.as_ref() == id
        })
        .collect::<Vec<_>>();
    session_entries.sort_by_key(|entry| entry.timestamp);

    if session_entries.is_empty() {
        if wants_json(shared) {
            ccusage_core::outln!("null")?;
        } else {
            ccusage_core::errln!("No session found with ID: {id}");
        }
        return Ok(());
    }

    let mut session = SessionAccumulator::default();
    for entry in &session_entries {
        session.add_entry(entry);
    }
    let mut summary = session.into_summary()?;
    summary.session_id = Some(id.to_string());

    if wants_json(shared) {
        if shared.requested_order() == Some(SortOrder::Desc) {
            // NOTE(2026-11-21): The entries are already sorted ascending. Flipping each equal-timestamp run back keeps ties in the order a stable descending sort gives.
            session_entries.reverse();
            for run in session_entries.chunk_by_mut(|left, right| left.timestamp == right.timestamp)
            {
                run.reverse();
            }
        }
        let output = json!({
            "sessionId": id,
            "totalCost": summary.total_cost,
            "totalTokens": summary.total_tokens(),
            "modelsUsed": summary.models_used,
            "modelBreakdowns": summary.model_breakdowns,
            "entries": session_entries.iter().map(|entry| json!({
                "timestamp": entry.source_timestamp,
                "inputTokens": entry.usage.input_tokens,
                "outputTokens": entry.usage.output_tokens,
                "cacheCreationTokens": entry.usage.cache_creation_input_tokens,
                "cacheReadTokens": entry.usage.cache_read_input_tokens,
                "model": entry.original_model.as_deref().unwrap_or("unknown"),
                "costUSD": entry.recorded_cost.unwrap_or(0.0),
            })).collect::<Vec<_>>(),
        });
        print_json_or_jq(output, shared.jq.as_deref(), shared.no_cost)?;
        return Ok(());
    }

    print_usage_table(
        &format!("Claude Code Session Usage - {id}"),
        "Session",
        &[summary],
        shared,
        false,
        None,
    )
}

fn run_blocks(args: BlocksArgs, project: Option<&str>) -> Result<()> {
    let session_duration = args.session_length;
    let shared = args.shared.clone();
    let now = shared.clock.now().into();
    let entries = load_entries(&shared, project)?;
    let mut blocks = identify_session_blocks(entries, session_duration, now);
    let window = ReportWindow::from_shared(&shared);
    filter_blocks_by_date(&mut blocks, &window);
    sort_blocks(&mut blocks, &shared.order.unwrap_or_default());

    if args.recent {
        let cutoff = now
            .checked_sub_millis(DEFAULT_RECENT_DAYS * MILLIS_PER_DAY)
            .unwrap_or(TimestampMs::UNIX_EPOCH);
        blocks.retain(|block| block.start_time >= cutoff || block.is_active);
    }

    if args.active {
        blocks.retain(|block| block.is_active);
    }

    let max_tokens = blocks
        .iter()
        .filter(|block| !block.is_gap && !block.is_active)
        .map(|block| block.token_counts.total())
        .max()
        .unwrap_or(0);

    let token_limit = args
        .token_limit
        .unwrap_or(ccusage_core::cli::TokenLimit::Max)
        .resolve(max_tokens);
    if wants_json(&shared) {
        let output = json!({
            "blocks": blocks.iter().map(|block| block_json(block, args.token_limit.and(token_limit), now)).collect::<Vec<_>>(),
        });
        print_json_or_jq(output, shared.jq.as_deref(), shared.no_cost)?;
        return Ok(());
    }

    if args.active && blocks.is_empty() {
        ccusage_core::outln!("No active session block found.")?;
        return Ok(());
    }
    if args.active && blocks.len() == 1 {
        print_active_block_detail(&blocks[0], token_limit, &shared, now, window.timezone())?;
        return Ok(());
    }
    print_blocks_table(&blocks, token_limit, &shared, now, window.timezone())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use ccusage_core::cli::CostMode;
    use ccusage_test_support::{Environment, fs_fixture};

    use super::*;

    #[test]
    fn claude_session_totals_only_include_entries_inside_date_window() {
        let rows = claude_session_rows(
            &[
                r#"{"timestamp":"2026-08-12T12:00:00.000Z","sessionId":"session-a","requestId":"request-old","costUSD":1.25,"message":{"id":"message-old","model":"claude-sonnet-4-20250514","usage":{"input_tokens":10,"output_tokens":2}}}"#,
                r#"{"timestamp":"2026-08-13T12:00:00.000Z","sessionId":"session-a","requestId":"request-new","costUSD":2.5,"message":{"id":"message-new","model":"claude-sonnet-4-20250514","usage":{"input_tokens":30,"output_tokens":4}}}"#,
            ]
            .join("\n"),
            SharedArgs {
                mode: CostMode::Display,
                since: Some("20260813".parse().unwrap()),
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                ..SharedArgs::default()
            },
        );

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].input_tokens, 30);
        assert_eq!(rows[0].output_tokens, 4);
        assert_eq!(rows[0].total_tokens(), 34);
        assert_eq!(rows[0].total_cost, 2.5);
    }

    #[test]
    fn claude_session_until_includes_the_boundary_date() {
        let rows = claude_session_rows(
            r#"{"timestamp":"2026-08-14T09:38:56.467Z","sessionId":"session-a","requestId":"request-boundary","costUSD":3.75,"message":{"id":"message-boundary","model":"claude-sonnet-4-20250514","usage":{"input_tokens":40,"output_tokens":5}}}"#,
            SharedArgs {
                mode: CostMode::Display,
                until: Some("20260814".parse().unwrap()),
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                ..SharedArgs::default()
            },
        );

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].total_tokens(), 45);
        assert_eq!(rows[0].total_cost, 3.75);
    }

    #[test]
    fn claude_session_date_window_uses_requested_timezone() {
        let rows = claude_session_rows(
            r#"{"timestamp":"2026-08-14T23:30:00.000Z","sessionId":"session-a","requestId":"request-timezone","costUSD":4.5,"message":{"id":"message-timezone","model":"claude-sonnet-4-20250514","usage":{"input_tokens":50,"output_tokens":6}}}"#,
            SharedArgs {
                mode: CostMode::Display,
                since: Some("20260815".parse().unwrap()),
                timezone: Some("Asia/Tokyo".parse().unwrap()),
                ..SharedArgs::default()
            },
        );

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].total_tokens(), 56);
        assert_eq!(rows[0].total_cost, 4.5);
    }

    fn claude_session_rows(log: &str, shared: SharedArgs) -> Vec<UsageSummary> {
        let fixture = fs_fixture!({
            "projects/project-a/session-a/chat.jsonl": log,
        });
        Environment::with_var("CLAUDE_CONFIG_DIR", fixture.root())
            .run(|| load_session_rows(&shared, None).unwrap())
    }

    #[test]
    fn claude_session_calculate_uses_each_deepseek_v4_event_timestamp() {
        ccusage_test_support::install_pricing_snapshots();
        let rows = claude_session_rows(
            &[
                r#"{"timestamp":"2026-08-15T23:59:59.000Z","sessionId":"session-a","requestId":"request-old","message":{"id":"message-old","model":"deepseek-v4-flash","usage":{"input_tokens":1000000,"output_tokens":0}}}"#,
                r#"{"timestamp":"2026-08-17T01:00:00.000Z","sessionId":"session-a","requestId":"request-peak","message":{"id":"message-peak","model":"deepseek-v4-pro","usage":{"input_tokens":1000000,"output_tokens":0}}}"#,
            ]
            .join("\n"),
            SharedArgs {
                mode: CostMode::Calculate,
                offline: true,
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                ..SharedArgs::default()
            },
        );

        assert_eq!(rows.len(), 1);
        assert!((rows[0].total_cost - 1.46).abs() < 1e-9);
    }
}
