use ccusage_core::{
    PricingMap, Result,
    cli::{AgentReportKind, CodexStatuslineArgs},
    format_currency, format_date_tz, format_number, parse_tz, print_json_or_jq, utc_now,
    wants_json,
};
use jiff::tz::TimeZone;

use super::{load_groups, report, resolve_codex_speed, resolve_session_id};

pub fn run_statusline(args: CodexStatuslineArgs) -> Result<()> {
    let mut shared = args.shared;
    let scope = if args.session_id.is_some() {
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
    let kind = if args.session_id.is_some() {
        AgentReportKind::Session
    } else {
        AgentReportKind::Daily
    };
    let pricing = PricingMap::load_with_overrides(
        shared.offline && !shared.no_offline,
        false,
        shared.pricing_overrides.iter(),
    );
    let mut groups = load_groups(&shared, kind)?;
    let session_id = if let Some(requested) = args.session_id {
        let id = resolve_session_id(&groups, &requested)?.to_string();
        groups.retain(|candidate, _| candidate == &id);
        Some(id)
    } else {
        None
    };
    let speed = resolve_codex_speed(args.codex_speed);
    if wants_json(&shared) {
        let mut output =
            report::report_from_groups(&groups, kind, &pricing, speed, shared.mode)["totals"]
                .take();
        output["scope"] = scope.into();
        if let Some(id) = session_id {
            output["sessionId"] = id.into();
        }
        return print_json_or_jq(output, shared.jq.as_deref(), shared.no_cost);
    }
    let tokens = groups.values().map(|group| group.total_tokens).sum();
    let label = match scope {
        "session" => "Session",
        "range" => "Usage",
        _ => "Today",
    };
    let mut output = format!("Codex | {label}: {} tokens", format_number(tokens));
    if !shared.no_cost {
        let cost = groups
            .values()
            .map(|group| report::calculate_group_cost(group, &pricing, speed))
            .fold(0.0, |total, cost| total + cost);
        output.push_str(&format!(" | Est. {}", format_currency(cost)));
        if !report::codex_missing_pricing_models(&groups, &pricing).is_empty() {
            output.push_str(" (incomplete pricing)");
        }
    }
    println!("{output}");
    Ok(())
}
