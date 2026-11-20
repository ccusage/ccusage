use ccusage_adapter_common::chunk_file_indexes_by_size;
use ccusage_core::{
    Color, ColumnKind, MILLIS_PER_DAY, Pricing, PricingMap, Result, SimpleTable, TimestampMs,
    attach_unpriced_models, cli, cli_error, color, date_range_bounds_ms, fast,
    format_breakdown_model_label, format_currency, format_models_multiline, format_number,
    format_rfc3339_millis, json_float, missing_pricing_model_for_token_total, model_aliases,
    parse_ts_timestamp, pricing, print_box_title, print_json_or_jq,
    print_missing_pricing_warnings_for_models, resolve_tz, sanitize_terminal_text, sum_costs,
    terminal_style, terminal_width, wants_json,
};

mod aggregate;
mod parser;
mod paths;
mod reader;
mod replay;
mod report;
mod source;
mod speed;
mod statusline;
mod types;

#[cfg(test)]
mod test_support;

use ccusage_adapter_common::source::ReportPricing;
use ccusage_core::cli::AgentCommandArgs;

pub use report::calculate_group_cost;
pub use source::SOURCE;
#[doc(hidden)]
pub use source::unified_row_from_group;
pub use speed::CodexSpeedPolicy;
use speed::resolve_codex_speed;
pub use statusline::run_statusline;
pub(crate) use types::CodexTimestampedUsage;
pub use types::{CodexGroup, CodexModelUsage, CodexServiceTier, CodexUsageBucket, TierBuckets};
use types::{CodexRawUsage, CodexTokenUsageEvent, merge_codex_service_tiers};

use report::{print_table_from_groups, report_from_groups, session_detail_json};

pub fn run(args: AgentCommandArgs) -> Result<()> {
    let shared = args.shared;
    let report_pricing = ReportPricing::for_load(&shared, None);
    let sources = paths::codex_usage_sources()?;
    let pricing = report_pricing.get();
    let diagnostics = ccusage_core::Diagnostics::for_report("codex", shared.clock);
    let (mut groups, _) = aggregate::load_groups_from_sources_with_diagnostics(
        &sources,
        &shared,
        args.kind,
        pricing,
        &diagnostics,
    )?;
    diagnostics.emit(shared.debug);
    let speed = resolve_codex_speed(args.options.codex_speed);
    if let Some(requested_id) = args.session_id {
        let session_id = resolve_session_id(&groups, &requested_id)?.to_string();
        if wants_json(&shared) {
            let output = session_detail_json(
                &session_id,
                &groups[&session_id],
                pricing,
                speed,
                shared.mode,
            );
            return print_json_or_jq(output, shared.jq.as_deref(), shared.no_cost);
        }
        groups.retain(|id, _| id == &session_id);
    }
    if wants_json(&shared) {
        let output = report_from_groups(&groups, args.kind, pricing, speed, shared.mode);
        return print_json_or_jq(output, shared.jq.as_deref(), shared.no_cost);
    }
    print_table_from_groups(&groups, args.kind, pricing, speed, &shared)
}

fn resolve_session_id<'a>(
    groups: &'a std::collections::BTreeMap<String, CodexGroup>,
    requested_id: &str,
) -> Result<&'a str> {
    let original_id = requested_id;
    let requested_id = normalize_session_id(original_id);
    let matches = groups
        .keys()
        .filter(|candidate| session_id_matches(candidate, requested_id))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [session_id] => Ok(session_id.as_str()),
        [] => Err(cli_error(format!(
            "No Codex session found with ID: {original_id}"
        ))),
        _ => Err(cli_error(format!(
            "Codex session ID '{original_id}' is ambiguous and matches {} sessions.",
            matches.len()
        ))),
    }
}

fn normalize_session_id(value: &str) -> &str {
    value
        .trim_prefix("codex://threads/")
        .trim_suffix(".zst")
        .trim_suffix(".jsonl")
}

fn rollout_identity(value: &str) -> Option<(&str, &str)> {
    let name = normalize_session_id(value)
        .rsplit('/')
        .next()?
        .strip_prefix("rollout-")?;
    let (timestamp, ids) = name.split_at_checked(20)?;
    if !timestamp
        .bytes()
        .enumerate()
        .all(|(index, byte)| match index {
            4 | 7 | 13 | 16 | 19 => byte == b'-',
            10 => byte == b'T',
            _ => byte.is_ascii_digit(),
        })
    {
        return None;
    }
    let (thread_id, rollout_id) = ids.split_once('_').unwrap_or((ids, ids));
    (is_codex_uuid(thread_id) && is_codex_uuid(rollout_id)).then_some((thread_id, rollout_id))
}

fn session_id_matches(candidate: &str, requested_id: &str) -> bool {
    let candidate = candidate.trim_suffix(".jsonl");
    if candidate == requested_id
        || candidate
            .rsplit_once('/')
            .map_or(candidate, |(_, file)| file)
            == requested_id
    {
        return true;
    }
    is_codex_uuid(requested_id)
        && candidate
            .strip_suffix(requested_id)
            .is_some_and(|prefix| prefix.ends_with('-'))
}

fn is_codex_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

#[cfg(test)]
mod tests;
