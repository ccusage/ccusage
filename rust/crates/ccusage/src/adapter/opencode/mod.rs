pub(crate) mod loader;
mod parser;
mod paths;
mod report;

pub(crate) use report::{
    agent_summary_json, first_column, report_json, summarize_entries, summary_period,
};

use crate::{
    Result, apply_session_names, cli::AgentCommandArgs, cli::AgentReportKind,
    filter_loaded_entries_by_date, print_json_or_jq, print_usage_table, sort_summaries, wants_json,
};

pub(crate) fn run(args: AgentCommandArgs) -> Result<()> {
    let shared = args.shared;
    let mut entries = loader::load_entries(&shared)?;
    filter_loaded_entries_by_date(&mut entries, &shared);
    let names = if args.kind == AgentReportKind::Session {
        loader::load_session_names(&shared)
    } else {
        std::collections::HashMap::new()
    };
    if wants_json(&shared) {
        return print_json_or_jq(
            report_json(&entries, args.kind, &shared.order, &names)?,
            shared.jq.as_deref(),
            shared.no_cost,
        );
    }
    let mut rows = summarize_entries(&entries, args.kind)?;
    apply_session_names(&mut rows, &names);
    sort_summaries(&mut rows, &shared.order, |row| summary_period(row));
    print_usage_table(
        "OpenCode Token Usage Report",
        first_column(args.kind),
        &rows,
        &shared,
        false,
        None,
    )?;
    Ok(())
}
