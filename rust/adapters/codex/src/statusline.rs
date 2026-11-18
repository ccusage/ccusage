use ccusage_core::{
    PricingMap, Result,
    cli::{AgentReportKind, CodexStatuslineArgs},
    format_currency, format_date_tz, format_number, parse_tz, print_json_or_jq, utc_now,
    wants_json,
};
use jiff::tz::TimeZone;
use serde_json::Value;

use super::{load_groups, report, resolve_codex_speed, resolve_session_id};

mod mcp;
mod native;

use native::NativeUsageReader;

pub fn run_statusline(args: CodexStatuslineArgs) -> Result<()> {
    if args.mcp {
        return mcp::serve(args);
    }
    let pricing = load_pricing(&args);
    let summary = summarize(&args, &pricing)?;
    if wants_json(&args.shared) {
        return print_json_or_jq(summary, args.shared.jq.as_deref(), args.shared.no_cost);
    }
    println!("{}", format_summary(&summary, args.shared.no_cost));
    Ok(())
}

fn load_pricing(args: &CodexStatuslineArgs) -> PricingMap {
    PricingMap::load_with_overrides(
        args.mcp || (args.shared.offline && !args.shared.no_offline),
        false,
        args.shared.pricing_overrides.iter(),
    )
}

fn summarize(args: &CodexStatuslineArgs, pricing: &PricingMap) -> Result<Value> {
    let mut shared = args.shared.clone();
    let requested_session = args.session_id.as_deref();
    let scope = if requested_session.is_some() {
        shared.since = None;
        shared.until = None;
        "session"
    } else if shared.since.is_some() || shared.until.is_some() {
        "range"
    } else {
        let timezone = parse_tz(shared.timezone.as_deref()).unwrap_or_else(TimeZone::system);
        let today = format_date_tz(utc_now(), Some(&timezone)).replace('-', "");
        shared.since = Some(today.clone());
        shared.until = Some(today);
        "today"
    };
    let kind = if requested_session.is_some() {
        AgentReportKind::Session
    } else {
        AgentReportKind::Daily
    };
    let mut groups = if let Some(requested) = requested_session {
        super::aggregate::load_session_groups(&shared, requested)?
    } else {
        load_groups(&shared, kind)?
    };
    let session_id = if let Some(requested) = requested_session {
        let thread = super::normalize_session_id(requested);
        let has_replacement = groups.keys().any(|id| {
            super::rollout_identity(id)
                .is_some_and(|(owner, rollout)| owner == thread && owner != rollout)
        });
        let id = if has_replacement {
            groups.retain(|candidate, _| {
                super::rollout_identity(candidate).is_some_and(|(owner, _)| owner == thread)
            });
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
