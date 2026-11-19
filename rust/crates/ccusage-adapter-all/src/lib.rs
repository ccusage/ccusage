mod loader;
mod report;
mod sources;
mod types;

use ccusage_core::*;

mod adapter {
    pub use ccusage_adapter_amp as amp;
    pub use ccusage_adapter_antigravity as antigravity;
    pub use ccusage_adapter_claude as claude;
    pub use ccusage_adapter_cline as cline;
    pub use ccusage_adapter_codebuff as codebuff;
    pub use ccusage_adapter_codex as codex;
    pub use ccusage_adapter_copilot as copilot;
    pub use ccusage_adapter_droid as droid;
    pub use ccusage_adapter_dsh as dsh;
    pub use ccusage_adapter_gemini as gemini;
    pub use ccusage_adapter_goose as goose;
    pub use ccusage_adapter_grok as grok;
    pub use ccusage_adapter_hermes as hermes;
    pub use ccusage_adapter_kilo as kilo;
    pub use ccusage_adapter_kimi as kimi;
    pub use ccusage_adapter_openclaw as openclaw;
    pub use ccusage_adapter_opencode as opencode;
    pub use ccusage_adapter_pi as pi;
    pub use ccusage_adapter_qwen as qwen;
    pub use ccusage_adapter_zcode as zcode;
}

use crate::{
    Result,
    cli::{AgentCommandArgs, AgentReportKind},
    print_json_or_jq, wants_json,
};

pub fn runner(agent: ccusage_cli::StandardAgent) -> fn(AgentCommandArgs) -> Result<()> {
    match agent {
        ccusage_cli::StandardAgent::Codex => adapter::codex::run,
        ccusage_cli::StandardAgent::Amp => adapter::amp::run,
        ccusage_cli::StandardAgent::Droid => adapter::droid::run,
        ccusage_cli::StandardAgent::Codebuff => adapter::codebuff::run,
        ccusage_cli::StandardAgent::Hermes => adapter::hermes::run,
        ccusage_cli::StandardAgent::Pi => adapter::pi::run,
        ccusage_cli::StandardAgent::Goose => adapter::goose::run,
        ccusage_cli::StandardAgent::Kilo => adapter::kilo::run,
        ccusage_cli::StandardAgent::Copilot => adapter::copilot::run,
        ccusage_cli::StandardAgent::Gemini => adapter::gemini::run,
        ccusage_cli::StandardAgent::Antigravity => adapter::antigravity::run,
        ccusage_cli::StandardAgent::Kimi => adapter::kimi::run,
        ccusage_cli::StandardAgent::Qwen => adapter::qwen::run,
        ccusage_cli::StandardAgent::OpenClaw => adapter::openclaw::run,
        ccusage_cli::StandardAgent::Grok => adapter::grok::run,
        ccusage_cli::StandardAgent::ZCode => adapter::zcode::run,
        ccusage_cli::StandardAgent::Cline => adapter::cline::run,
        ccusage_cli::StandardAgent::Dsh => adapter::dsh::run,
        ccusage_cli::StandardAgent::OpenCode => run_opencode,
    }
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    let kind = args.kind;
    let shared = args.shared;
    let sources = args.sources;
    let include_agents = args.by_agent;
    let start_of_week = args.start_of_week;
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
    let result = loader::load_rows(kind, &shared, &sources, start_of_week)?;
    emit_diagnostics(&result.diagnostics, &shared, &sources);
    if wants_json(&shared) {
        let output = report::report_json_with_agents(&result.rows, kind, include_agents);
        return print_json_or_jq(output, shared.jq.as_deref(), shared.no_cost);
    }
    report::print_table(&result.rows, kind, &shared, &result.detected_agents)
}

/// Runs `ccusage opencode`, leaving Claude Agent turns to the Claude source
/// unless `--include-claude-agent` keeps them.
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

/// Prints each source's skips with the options that source loaded with.
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
