use std::sync::Arc;

use ccusage_core::{
    BucketKind, DEFAULT_RECENT_DAYS, MILLIS_PER_DAY, ReportWindow, Result, SessionAccumulator,
    TimestampMs, UsageSummary,
    cli::{BlocksArgs, DailyArgs, SessionArgs, SharedArgs, SortOrder, WeekDay, WeeklyArgs},
    cli_error,
    fast::FxHashMap,
    print_json_or_jq, print_usage_table, sort_summaries, summarize_by_key,
    summarize_summaries_by_bucket, totals_json, utc_now, wants_json,
};
use ccusage_report::{group_project_output, session_summary_json, summary_json};
use serde_json::json;

use crate::{
    blocks::{
        block_json, filter_blocks_by_date, identify_session_blocks, print_active_block_detail,
        print_blocks_table, sort_blocks,
    },
    load_daily_summaries, load_entries, load_entries_since,
};

pub fn run_daily(args: DailyArgs) -> Result<()> {
    let shared = args.shared.clone();
    let mut rows = load_daily_summaries(
        &shared,
        args.project.as_deref(),
        args.instances || args.project.is_some(),
    )?;
    sort_summaries(&mut rows, &shared.order, |row| {
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

pub fn run_bucket(shared: SharedArgs, kind: BucketKind) -> Result<()> {
    run_bucket_with_start(shared, kind, WeekDay::Sunday)
}

fn run_bucket_with_start(shared: SharedArgs, kind: BucketKind, start: WeekDay) -> Result<()> {
    let entries = load_entries_since(&shared, None)?;
    let mut daily = summarize_by_key(
        &entries,
        |entry| entry.date.clone(),
        |key| (key.to_string(), None),
    )?;
    sort_summaries(&mut daily, &shared.order, |row| {
        row.date.as_deref().unwrap_or_default()
    });

    let mut buckets = summarize_summaries_by_bucket(&daily, kind, start);
    sort_summaries(&mut buckets, &shared.order, |row| match kind {
        BucketKind::Monthly => row.month.as_deref().unwrap_or_default(),
        BucketKind::Weekly => row.week.as_deref().unwrap_or_default(),
    });

    if wants_json(&shared) {
        let key = match kind {
            BucketKind::Monthly => "monthly",
            BucketKind::Weekly => "weekly",
        };
        let output = json!({
            key: buckets.iter().map(summary_json).collect::<Vec<_>>(),
            "totals": totals_json(&buckets),
        });
        print_json_or_jq(output, shared.jq.as_deref(), shared.no_cost)?;
        return Ok(());
    }

    let (title, col) = match kind {
        BucketKind::Monthly => ("Claude Code Token Usage Report - Monthly", "Month"),
        BucketKind::Weekly => ("Claude Code Token Usage Report - Weekly", "Week"),
    };
    print_usage_table(title, col, &buckets, &shared, false, None)?;
    Ok(())
}

pub fn run_weekly(args: WeeklyArgs) -> Result<()> {
    run_bucket_with_start(args.shared, BucketKind::Weekly, args.start_of_week)
}

pub fn run_session(args: SessionArgs) -> Result<()> {
    let shared = args.shared.clone();
    if let Some(id) = args.id {
        return run_session_id(&id, &shared);
    }

    let mut session_shared = shared.clone();
    session_shared.order = SortOrder::Desc;
    let mut rows = load_session_rows(&session_shared)?;
    rows.retain(|row| {
        row.input_tokens + row.output_tokens + row.cache_creation_tokens + row.cache_read_tokens > 0
    });
    rows.sort_by(|a, b| match session_shared.order {
        SortOrder::Asc => a.total_cost.total_cmp(&b.total_cost),
        SortOrder::Desc => b.total_cost.total_cmp(&a.total_cost),
    });

    if wants_json(&session_shared) {
        let output = json!({
            "sessions": rows.iter().map(session_summary_json).collect::<Vec<_>>(),
            "totals": totals_json(&rows),
        });
        print_json_or_jq(output, session_shared.jq.as_deref(), session_shared.no_cost)?;
        return Ok(());
    }

    print_usage_table(
        "Claude Code Token Usage Report - By Session",
        "Session",
        &rows,
        &session_shared,
        false,
        None,
    )?;
    Ok(())
}

fn load_session_rows(shared: &SharedArgs) -> Result<Vec<UsageSummary>> {
    let entries = load_entries_since(shared, None)?;
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

fn run_session_id(id: &str, shared: &SharedArgs) -> Result<()> {
    let entries = load_entries_since(shared, None)?;
    let mut session_entries = entries
        .into_iter()
        .filter(|entry| {
            entry.data.session_id.as_deref() == Some(id) || entry.session_id.as_ref() == id
        })
        .collect::<Vec<_>>();
    session_entries.sort_by_key(|entry| entry.timestamp);

    if session_entries.is_empty() {
        if wants_json(shared) {
            println!("null");
        } else {
            eprintln!("No session found with ID: {id}");
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
        let output = json!({
            "sessionId": id,
            "totalCost": summary.total_cost,
            "totalTokens": summary.total_tokens(),
            "modelsUsed": summary.models_used,
            "modelBreakdowns": summary.model_breakdowns,
            "entries": session_entries.iter().map(|entry| json!({
                "timestamp": entry.data.timestamp,
                "inputTokens": entry.data.message.usage.input_tokens,
                "outputTokens": entry.data.message.usage.output_tokens,
                "cacheCreationTokens": entry.data.message.usage.cache_creation_input_tokens,
                "cacheReadTokens": entry.data.message.usage.cache_read_input_tokens,
                "model": entry.data.message.model.as_deref().unwrap_or("unknown"),
                "costUSD": entry.data.cost_usd.unwrap_or(0.0),
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

pub fn run_blocks(args: BlocksArgs) -> Result<()> {
    if args.session_length <= 0.0 {
        return Err(cli_error("Session length must be a positive number"));
    }
    let shared = args.shared.clone();
    let now = utc_now();
    let entries = load_entries(&shared, None)?;
    let mut blocks = identify_session_blocks(entries, args.session_length, now);
    let window = ReportWindow::from_shared(&shared);
    filter_blocks_by_date(&mut blocks, &window);
    sort_blocks(&mut blocks, &shared.order);

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

    if wants_json(&shared) {
        let output = json!({
            "blocks": blocks.iter().map(|block| block_json(block, args.token_limit.as_deref(), max_tokens, now)).collect::<Vec<_>>(),
        });
        print_json_or_jq(output, shared.jq.as_deref(), shared.no_cost)?;
        return Ok(());
    }

    if args.active && blocks.is_empty() {
        println!("No active session block found.");
        return Ok(());
    }
    if args.active && blocks.len() == 1 {
        print_active_block_detail(
            &blocks[0],
            args.token_limit.as_deref(),
            max_tokens,
            &shared,
            now,
            window.timezone(),
        )?;
        return Ok(());
    }
    print_blocks_table(
        &blocks,
        args.token_limit.as_deref(),
        max_tokens,
        &shared,
        now,
        window.timezone(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use ccusage_core::cli::CostMode;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

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
                since: Some("20260813".to_string()),
                timezone: Some("UTC".to_string()),
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
                until: Some("20260814".to_string()),
                timezone: Some("UTC".to_string()),
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
                since: Some("20260815".to_string()),
                timezone: Some("Asia/Tokyo".to_string()),
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
        let _env = EnvVarGuard::set("CLAUDE_CONFIG_DIR", fixture.root());
        load_session_rows(&shared).unwrap()
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
                timezone: Some("UTC".to_string()),
                ..SharedArgs::default()
            },
        );

        assert_eq!(rows.len(), 1);
        assert!((rows[0].total_cost - 1.46).abs() < 1e-9);
    }
}
