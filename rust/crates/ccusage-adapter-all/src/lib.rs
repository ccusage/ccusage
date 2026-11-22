mod loader;
mod report;
mod sources;
mod types;

use ccusage_cli::{AgentCommand, StandardAgent};
use ccusage_core::{
    Color, ColumnKind, Diagnostics, LoadedEntry, ModelBreakdown, PricingMap, Result, SimpleTable,
    Skip, SkipKind, Totals, TotalsRow, USAGE_COMPACT_WIDTH_THRESHOLD, attach_estimated_models,
    attach_unpriced_models, cli, cli_error, color, fast, first_column,
    format_breakdown_model_label, format_currency, format_models_multiline, format_number,
    json_float, log_level, output, print_box_title, print_estimated_pricing_notes_for_models,
    print_json_or_jq, print_missing_pricing_warnings_for_models, progress,
    should_use_compact_layout, terminal_style, terminal_width, wants_json,
};

mod adapter {
    pub(crate) use ccusage_adapter_amp as amp;
    pub(crate) use ccusage_adapter_antigravity as antigravity;
    pub(crate) use ccusage_adapter_claude as claude;
    pub(crate) use ccusage_adapter_cline as cline;
    pub(crate) use ccusage_adapter_codebuff as codebuff;
    pub(crate) use ccusage_adapter_codex as codex;
    pub(crate) use ccusage_adapter_copilot as copilot;
    pub(crate) use ccusage_adapter_droid as droid;
    pub(crate) use ccusage_adapter_dsh as dsh;
    pub(crate) use ccusage_adapter_gemini as gemini;
    pub(crate) use ccusage_adapter_goose as goose;
    pub(crate) use ccusage_adapter_grok as grok;
    pub(crate) use ccusage_adapter_hermes as hermes;
    pub(crate) use ccusage_adapter_kilo as kilo;
    pub(crate) use ccusage_adapter_kimi as kimi;
    pub(crate) use ccusage_adapter_openclaw as openclaw;
    pub(crate) use ccusage_adapter_opencode as opencode;
    pub(crate) use ccusage_adapter_pi as pi;
    pub(crate) use ccusage_adapter_qwen as qwen;
    pub(crate) use ccusage_adapter_zcode as zcode;
}

use crate::cli::{AgentCommandArgs, AgentReportKind};

pub fn runner(agent: StandardAgent, command: AgentCommand) -> Result<()> {
    match (agent, command) {
        (StandardAgent::Codex, AgentCommand::Statusline(args)) => {
            adapter::codex::run_statusline(args)
        }
        (agent, AgentCommand::Statusline(_)) => Err(cli_error(format!(
            "The statusline runner is unavailable for {}.",
            agent.spec().name
        ))),
        (agent, AgentCommand::Report(args)) => report_runner(agent)(args),
    }
}

fn report_runner(agent: StandardAgent) -> fn(AgentCommandArgs) -> Result<()> {
    match agent {
        StandardAgent::Codex => adapter::codex::run,
        StandardAgent::Amp => adapter::amp::run,
        StandardAgent::Droid => adapter::droid::run,
        StandardAgent::Codebuff => adapter::codebuff::run,
        StandardAgent::Hermes => adapter::hermes::run,
        StandardAgent::Pi => adapter::pi::run,
        StandardAgent::Goose => adapter::goose::run,
        StandardAgent::Kilo => adapter::kilo::run,
        StandardAgent::Copilot => adapter::copilot::run,
        StandardAgent::Gemini => adapter::gemini::run,
        StandardAgent::Antigravity => adapter::antigravity::run,
        StandardAgent::Kimi => adapter::kimi::run,
        StandardAgent::Qwen => adapter::qwen::run,
        StandardAgent::OpenClaw => adapter::openclaw::run,
        StandardAgent::Grok => adapter::grok::run,
        StandardAgent::ZCode => adapter::zcode::run,
        StandardAgent::Cline => adapter::cline::run,
        StandardAgent::Dsh => adapter::dsh::run,
        StandardAgent::OpenCode => run_opencode,
    }
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    let kind = args.kind;
    let shared = args.shared;
    let sources = args.sources;
    let include_agents = args.by_agent;
    let start_of_week = args.start_of_week;
    let note = sources
        .get("claude")
        .is_some_and(|source| source.options.claude_project.is_some())
        .then_some("The project filter applies only to Claude Code usage.");
    if let Some(sections) = args.sections {
        let sections = requested_sections(kind, sections);
        let result = loader::load_sections(&sections, &shared, &sources, start_of_week)?;
        emit_diagnostics(&result.diagnostics, &shared, &sources);
        if wants_json(&shared) {
            return report::print_sections_report_json(
                &result.sections,
                kind,
                include_agents,
                shared.jq.as_deref(),
                shared.no_cost,
                note,
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
    } else {
        let result = loader::load_rows(kind, &shared, &sources, start_of_week)?;
        emit_diagnostics(&result.diagnostics, &shared, &sources);
        if wants_json(&shared) {
            let mut output = report::report_json_with_agents(&result.rows, kind, include_agents);
            if let Some(note) = note {
                output["notes"] = serde_json::json!([note]);
            }
            return print_json_or_jq(output, shared.jq.as_deref(), shared.no_cost);
        }
        report::print_table(&result.rows, kind, &shared, &result.detected_agents)?;
    }
    if let Some(note) = note {
        ccusage_core::errln!("INFO  {note}");
    }
    Ok(())
}

fn run_opencode(args: AgentCommandArgs) -> Result<()> {
    let (entries, claude_diagnostics) = loader::load_opencode_entries(
        &args.shared,
        args.kind,
        args.include_claude_agent,
        args.sources.get("claude"),
    )?;
    if let Some(diagnostics) = claude_diagnostics {
        diagnostics.emit(
            args.sources
                .get("claude")
                .map_or(args.shared.debug, |source| source.shared.debug),
        );
    }
    adapter::opencode::print_report(&args, entries)
}

fn emit_diagnostics(
    diagnostics: &[Diagnostics],
    shared: &cli::SharedArgs,
    sources: &std::collections::BTreeMap<String, cli::SourceArgs>,
) {
    for diagnostics in diagnostics {
        let shared = sources
            .get(diagnostics.source())
            .map_or(shared, |source| &source.shared);
        diagnostics.emit(shared.debug);
    }
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
mod tests;
