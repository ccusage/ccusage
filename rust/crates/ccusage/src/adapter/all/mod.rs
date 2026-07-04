mod loader;
mod report;
mod types;

use crate::{
    Result,
    cli::{AgentCommandArgs, AgentReportKind},
    print_json_or_jq, wants_json,
};

pub(crate) fn run(args: AgentCommandArgs) -> Result<()> {
    let kind = args.kind;
    let shared = args.shared;
    let include_agents = args.by_agent;
    if let Some(sections) = args.sections {
        let sections = requested_sections(kind, sections);
        let result = loader::load_sections(&sections, &shared)?;
        if wants_json(&shared) {
            return report::print_sections_report_json(
                &result.sections,
                kind,
                include_agents,
                shared.jq.as_deref(),
                shared.no_cost,
            );
        }
        for (section_kind, rows) in &result.sections {
            report::print_table(
                rows,
                *section_kind,
                &shared,
                result.detected_agents_for(*section_kind),
            )?;
        }
        return Ok(());
    }
    let result = loader::load_rows(kind, &shared)?;
    if wants_json(&shared) {
        let output = report::report_json_with_agents(&result.rows, kind, include_agents);
        return print_json_or_jq(output, shared.jq.as_deref(), shared.no_cost);
    }
    report::print_table(&result.rows, kind, &shared, &result.detected_agents)
}

fn requested_sections(
    command_kind: AgentReportKind,
    sections: Vec<AgentReportKind>,
) -> Vec<AgentReportKind> {
    let mut requested = vec![command_kind];
    for section in [
        AgentReportKind::Daily,
        AgentReportKind::Weekly,
        AgentReportKind::Monthly,
        AgentReportKind::Session,
    ] {
        if section != command_kind && sections.contains(&section) {
            requested.push(section);
        }
    }
    requested
}

#[cfg(test)]
use loader::{aggregate_rows, codex_group_row, load_agent_rows_parallel, load_rows, load_sections};
#[cfg(test)]
use report::{
    all_report_title, all_table_columns, all_table_row, report_json, report_json_with_agents,
    sections_report_json,
};
#[cfg(test)]
use types::{AgentLoadSpec, AgentRows, AllRow};

#[cfg(test)]
mod tests;
