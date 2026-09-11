//! Loads per-frame token usage from the Claude Science metadata database.

use std::sync::Arc;

use ccusage_core::{
    LoadedEntry, PricingMap, Result, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    calculate_cost_for_usage_at, cli_error, debug_log, format_date_tz, format_rfc3339_millis,
    parse_tz,
};
use sqlite::{Connection, State};

use crate::{
    cli::{CostMode, SharedArgs},
    paths,
};

/// Per-frame usage record straight from the database.
pub(super) struct FrameUsage {
    id: String,
    session_id: String,
    model: String,
    project_name: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    recorded_cost_usd: Option<f64>,
    timestamp_ms: TimestampMs,
}

/// Loads Claude Science frame usage from all discovered databases.
pub fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent("Claude Science"),
        shared.json,
        || load_entries_inner(shared, pricing),
    )
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let timezone = parse_tz(shared.timezone.as_deref());
    let mut entries = Vec::new();
    for path in paths::database_paths()? {
        match read_frames(&path) {
            Ok(frames) => {
                for frame in frames {
                    entries.push(frame_to_loaded(
                        frame,
                        timezone.as_ref(),
                        shared.mode,
                        pricing,
                    ));
                }
            }
            Err(error) => {
                // One unreadable or incompatible database must not hide the
                // usage recorded in the others.
                debug_log(shared, format!("skipping database {path:?}: {error}"));
            }
        }
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

fn read_frames(path: &std::path::Path) -> Result<Vec<FrameUsage>> {
    let connection = Connection::open_with_flags(
        path,
        sqlite::OpenFlags::new().with_read_only().with_no_mutex(),
    )
    .map_err(|error| cli_error(format!("failed to open {}: {error}", path.display())))?;
    let query = if projects_table_exists(&connection) {
        "SELECT frames.id, COALESCE(frames.root_frame_id, frames.id), frames.model, \
         frames.input_tokens, frames.output_tokens, frames.cache_read_tokens, \
         frames.cache_write_tokens, frames.total_cost, frames.updated_at, projects.name \
         FROM frames LEFT JOIN projects ON projects.id = frames.project_id \
         WHERE frames.input_tokens IS NOT NULL AND frames.output_tokens IS NOT NULL"
    } else {
        "SELECT frames.id, COALESCE(frames.root_frame_id, frames.id), frames.model, \
         frames.input_tokens, frames.output_tokens, frames.cache_read_tokens, \
         frames.cache_write_tokens, frames.total_cost, frames.updated_at, NULL \
         FROM frames \
         WHERE frames.input_tokens IS NOT NULL AND frames.output_tokens IS NOT NULL"
    };
    let mut statement = connection
        .prepare(query)
        .map_err(|error| cli_error(format!("{}: {error}", path.display())))?;
    let mut frames = Vec::new();
    while let State::Row = statement
        .next()
        .map_err(|error| cli_error(format!("{}: {error}", path.display())))?
    {
        let timestamp_ms = statement
            .read::<Option<i64>, _>(8)
            .ok()
            .flatten()
            .unwrap_or_default();
        if timestamp_ms <= 0 {
            continue;
        }
        frames.push(FrameUsage {
            id: statement
                .read::<String, _>(0)
                .map_err(|error| cli_error(format!("{}: {error}", path.display())))?,
            session_id: statement
                .read::<String, _>(1)
                .map_err(|error| cli_error(format!("{}: {error}", path.display())))?,
            model: statement
                .read::<Option<String>, _>(2)
                .ok()
                .flatten()
                .unwrap_or_default(),
            input_tokens: non_negative(statement.read::<Option<i64>, _>(3).ok().flatten()),
            output_tokens: non_negative(statement.read::<Option<i64>, _>(4).ok().flatten()),
            cache_read_tokens: non_negative(statement.read::<Option<i64>, _>(5).ok().flatten()),
            cache_write_tokens: non_negative(statement.read::<Option<i64>, _>(6).ok().flatten()),
            recorded_cost_usd: statement.read::<Option<f64>, _>(7).ok().flatten(),
            timestamp_ms: TimestampMs::from_millis(timestamp_ms),
            project_name: statement.read::<Option<String>, _>(9).ok().flatten(),
        });
    }
    Ok(frames)
}

fn non_negative(value: Option<i64>) -> u64 {
    value.unwrap_or_default().max(0) as u64
}

fn projects_table_exists(connection: &Connection) -> bool {
    let Ok(mut statement) = connection
        .prepare("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'projects'")
    else {
        return false;
    };
    if statement.next().is_err() {
        return false;
    }
    matches!(statement.read::<i64, _>(0), Ok(count) if count == 1)
}

/// Claude Science may route models through a proxy that prefixes model names
/// with a routing namespace (for example `cs-switch-direct:claude-sonnet-4-5`).
/// Strip the namespace so downstream pricing lookups see the bare model name.
pub(crate) fn normalize_model(model: &str) -> &str {
    model.split_once(':').map_or(model, |(_, rest)| rest)
}

fn frame_to_loaded(
    frame: FrameUsage,
    timezone: Option<&jiff::tz::TimeZone>,
    mode: CostMode,
    pricing: &PricingMap,
) -> LoadedEntry {
    let usage = TokenUsageRaw {
        input_tokens: frame.input_tokens,
        output_tokens: frame.output_tokens,
        cache_creation_input_tokens: frame.cache_write_tokens,
        cache_read_input_tokens: frame.cache_read_tokens,
        speed: None,
        cache_creation: None,
    };
    let model = normalize_model(&frame.model).to_string();
    let missing_pricing_model = ccusage_core::missing_pricing_model_for_usage(
        Some(&model),
        usage,
        frame.recorded_cost_usd,
        mode,
        Some(pricing),
    );
    let cost = calculate_cost_for_usage_at(
        Some(&model),
        usage,
        frame.recorded_cost_usd,
        Some(frame.timestamp_ms),
        mode,
        Some(pricing),
    );
    let data = UsageEntry {
        session_id: Some(frame.session_id.clone()),
        timestamp: format_rfc3339_millis(frame.timestamp_ms),
        version: None,
        message: UsageMessage {
            usage,
            model: Some(model.clone()),
            id: Some(frame.id.clone()),
        },
        cost_usd: None,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };
    LoadedEntry {
        date: format_date_tz(frame.timestamp_ms, timezone),
        timestamp: frame.timestamp_ms,
        project: Arc::from(frame.project_name.as_deref().unwrap_or("claude-science")),
        session_id: Arc::from(frame.session_id),
        project_path: Arc::from("Claude Science"),
        cost,
        extra_total_tokens: 0,
        credits: None,
        message_count: None,
        model: Some(model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_proxy_prefixes() {
        assert_eq!(
            normalize_model("cs-switch-direct:claude-sonnet-4-5"),
            "claude-sonnet-4-5"
        );
        assert_eq!(normalize_model("claude-sonnet-4-5"), "claude-sonnet-4-5");
    }
}
