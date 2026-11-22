use ccusage_adapter_common::source::ReportPricing;
use ccusage_core::{
    Diagnostics, Result,
    cli::{AgentReportKind, CodexStatuslineArgs},
    date_at, format_currency, format_number, print_json_or_jq, resolve_tz, wants_json,
};
use serde_json::Value;

use super::{
    aggregate::{load_groups_from_file_selection, select_session_files},
    paths, report, resolve_codex_speed, resolve_session_id,
};

mod mcp;
mod native;

use native::NativeUsageReader;

pub fn run_statusline(args: CodexStatuslineArgs) -> Result<()> {
    if args.mcp {
        return mcp::serve(args);
    }
    let pricing = ReportPricing::for_load(&args.shared, None);
    let summary = summarize(&args, &pricing)?;
    if wants_json(&args.shared) {
        return print_json_or_jq(summary, args.shared.jq.as_deref(), args.shared.no_cost);
    }
    ccusage_core::outln!("{}", format_summary(&summary, args.shared.no_cost))?;
    Ok(())
}

fn summarize(args: &CodexStatuslineArgs, pricing: &ReportPricing<'_>) -> Result<Value> {
    let mut shared = args.shared.clone();
    let requested_session = args.session_id.as_deref();
    let scope = if requested_session.is_some() {
        shared.since = None;
        shared.until = None;
        "session"
    } else if shared.since.is_some() || shared.until.is_some() {
        "range"
    } else {
        let timezone = resolve_tz(shared.timezone.as_ref());
        let today = date_at(shared.clock.now().into(), &timezone).into();
        shared.since = Some(today);
        shared.until = Some(today);
        "today"
    };
    let kind = if requested_session.is_some() {
        AgentReportKind::Session
    } else {
        AgentReportKind::Daily
    };
    let diagnostics = Diagnostics::new("codex");
    let pricing = pricing.get();
    let mut groups = if let Some(requested) = requested_session {
        let files =
            paths::collect_deduped_codex_usage_files(&paths::codex_usage_sources()?, &diagnostics);
        let selected = select_session_files(&files, requested, false);
        load_groups_from_file_selection(&files, &selected, &shared, kind, pricing, &diagnostics)?
    } else {
        super::aggregate::load_groups_with_diagnostics(&shared, kind, pricing, &diagnostics)?.0
    };
    diagnostics.emit(shared.debug);
    let session_id = if let Some(requested) = requested_session {
        let thread = super::normalize_session_id(requested);
        let has_replacement = groups.keys().any(|id| {
            super::rollout_identity(id)
                .is_some_and(|(owner, rollout)| owner == thread && owner != rollout)
        });
        let id = if has_replacement {
            thread.to_string()
        } else {
            let id = resolve_session_id(&groups, requested)?.to_string();
            groups.retain(|candidate, _| candidate == &id);
            id
        };
        Some(id)
    } else {
        None
    };
    let speed = resolve_codex_speed(args.codex_speed);
    let mut output =
        report::report_from_groups(&groups, kind, pricing, speed, shared.mode)["totals"].take();
    output["scope"] = scope.into();
    if let Some(id) = session_id {
        output["sessionId"] = id.into();
    }
    Ok(output)
}

fn format_summary(summary: &Value, no_cost: bool) -> String {
    let tokens = summary["totalTokens"].as_u64().unwrap_or_default();
    let label = match summary["scope"].as_str() {
        Some("session") => "Session",
        Some("range") => "Usage",
        _ => "Today",
    };
    let mut output = format!("Codex | {label}: {} tokens", format_number(tokens));
    if !no_cost {
        let cost = summary["costUSD"].as_f64().unwrap_or_default();
        output.push_str(&format!(" | Est. {}", format_currency(cost)));
        if summary["unpricedModels"]
            .as_array()
            .is_some_and(|models| !models.is_empty())
        {
            output.push_str(" (incomplete pricing)");
        }
    }
    output
}
