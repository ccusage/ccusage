mod cache;
mod loader;
mod report;
mod types;

use crate::{Result, cli::AgentCommandArgs, print_json_or_jq, wants_json};

pub(crate) fn run(args: AgentCommandArgs) -> Result<()> {
    let kind = args.kind;
    let shared = args.shared;
    let result = loader::load_rows(kind, &shared)?;
    if wants_json(&shared) {
        return print_json_or_jq(
            report::report_json(&result.rows, kind),
            shared.jq.as_deref(),
            shared.no_cost,
        );
    }
    report::print_table(&result.rows, kind, &shared, &result.detected_agents)
}

#[cfg(test)]
use loader::{aggregate_rows, codex_group_row, load_agent_rows_parallel};
#[cfg(test)]
use report::{all_report_title, all_table_columns, all_table_row, report_json};
#[cfg(test)]
use types::{AgentLoadSpec, AgentRows, AllRow};

#[cfg(test)]
mod tests;
