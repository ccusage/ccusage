use ccusage_adapter_common::chunk_file_indexes_by_size;
use ccusage_core::{
    Color, ColumnKind, MILLIS_PER_DAY, Pricing, PricingMap, Result, SimpleTable, TimestampMs,
    attach_estimated_models, attach_unpriced_models, cli, cli_error, color, date_range_bounds_ms,
    fast, format_breakdown_model_label, format_currency, format_models_multiline, format_number,
    format_rfc3339_millis, json_float, model_aliases, parse_ts_timestamp, pricing, print_box_title,
    print_estimated_pricing_notes_for_models, print_json_or_jq,
    print_missing_pricing_warnings_for_models, resolve_tz, sanitize_terminal_text, sum_costs,
    terminal_style, terminal_width, wants_json,
};

mod aggregate;
mod parser;
mod paths;
mod reader;
mod replay;
mod report;
mod session_id;
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
use session_id::{
    is_codex_uuid, normalize_session_id, resolve_session_id, rollout_identity, session_id_matches,
};

pub fn run(args: AgentCommandArgs) -> Result<()> {
    let shared = args.shared;
    let report_pricing = ReportPricing::for_load(&shared, None);
    let sources = paths::codex_usage_sources()?;
    let pricing = report_pricing.get();
    let diagnostics = ccusage_core::Diagnostics::new("codex");
    let (mut groups, _) = aggregate::load_groups_from_sources_with_diagnostics(
        &sources,
        &shared,
        args.kind,
        args.start_of_week,
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

#[cfg(test)]
mod tests;
