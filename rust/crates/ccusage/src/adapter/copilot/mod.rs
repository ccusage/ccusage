mod loader;
mod parser;
mod paths;
mod report;

use std::sync::OnceLock;

use crate::cli::AgentCommandArgs;
use crate::{
    PricingMap, Result, filter_loaded_entries_by_date, print_json_or_jq, print_usage_table,
    sort_summaries, wants_json,
};

pub(crate) use loader::load_entries;
#[cfg(test)]
pub(crate) use paths::COPILOT_CONFIG_DIR_ENV;
pub(crate) use report::{report_from_rows, summarize_entries};

/// Returns `true` if any GitHub Copilot session-state file exists on disk
/// (under `<COPILOT_CONFIG_DIR>/session-state/<uuid>/events.jsonl`),
/// regardless of whether parsing succeeds or whether the active
/// `--since`/`--until` filter excludes every entry.
///
/// Used by the cross-source aggregator (`adapter::all::loader::load_copilot_rows`)
/// to keep "copilot" in the `Detected:` report header even when the date
/// filter narrows the row set to empty — mirroring the
/// `qwen::has_data()` sentinel pattern. This is required because
/// `load_entries_inner` pre-filters by date before its content-keyed dedup
/// HashMap collapse (see `loader.rs` — the credit-only dedup key omits
/// `event_id`, so filtering after the HashMap could let a content-key
/// collision drop the in-range row). With the pre-filter in place,
/// `!entries.is_empty()` no longer reflects on-disk presence; the
/// `has_data()` sentinel restores that signal.
pub(crate) fn has_data() -> bool {
    paths::has_any_session_state_event_file()
}

pub(crate) fn run(args: AgentCommandArgs) -> Result<()> {
    let shared = args.shared;
    let pricing = PricingMap::load_with_overrides(
        shared.offline,
        crate::log_level() != Some(0),
        shared.pricing_overrides.iter(),
    );
    // `load_entries` triggers `warn_about_inert_legacy_env_vars_once`
    // internally, so direct `ccusage copilot ...` and the cross-source
    // aggregator (`ccusage daily --all`) both surface the warning via the
    // same code path. The `OnceLock` inside that helper keeps the warning
    // to a single emission per process — including the case where both
    // direct invocation and the aggregator run in the same process (they
    // don't today, but the gate is cheap insurance).
    let mut entries = load_entries(&shared, &pricing)?;
    filter_loaded_entries_by_date(&mut entries, &shared);
    let mut rows = summarize_entries(&entries, args.kind)?;
    sort_summaries(&mut rows, &shared.order, |row| {
        crate::adapter::opencode::summary_period(row)
    });
    if wants_json(&shared) {
        return print_json_or_jq(
            report_from_rows(&rows, args.kind),
            shared.jq.as_deref(),
            shared.no_cost,
        );
    }
    if rows.is_empty() {
        eprintln!("{}", empty_usage_message());
        return Ok(());
    }
    print_usage_table(
        "GitHub Copilot CLI Token Usage Report",
        crate::adapter::opencode::first_column(args.kind),
        &rows,
        &shared,
        false,
        None,
    )?;
    Ok(())
}

/// Process-wide OnceLock that guards the legacy-OTel-env-var deprecation
/// warning. Initialized on first call; subsequent calls are no-ops. This is
/// what keeps the warning at most one emission per process even when
/// `load_entries` is hit through both `copilot::run` and the
/// cross-source `all` aggregator.
static LEGACY_OTEL_WARNING_EMITTED: OnceLock<()> = OnceLock::new();

/// Emit a one-shot warning to stderr when the user still has any of the
/// legacy OTel environment variables set. ccusage no longer reads OTel
/// exports (the production loader uses the configured `session-state/` directly),
/// so silently ignoring `COPILOT_OTEL_FILE_EXPORTER_PATH` / `COPILOT_OTEL_DEDUP`
/// / `COPILOT_PREFER_OTEL` would surprise users who configured them on a
/// previous ccusage version. Routed through the same `log_level`-aware
/// gate as the rest of the warning surface so `LOG_LEVEL=0` silences it.
///
/// Called from `copilot::load_entries` (NOT only from `copilot::run`) so
/// the warning ALSO fires when Copilot is loaded indirectly via the
/// cross-source aggregator (`ccusage daily --all`). The process-wide
/// `OnceLock` ensures at most one emission per process.
pub(crate) fn warn_about_inert_legacy_env_vars_once() {
    // Order matters: check the guards BEFORE consuming the OnceLock, so
    // the one-shot is only burned when we actually emit a warning. If we
    // set the lock first (the obvious order), the first `load_entries`
    // of the process always consumes it — even with no legacy vars set —
    // and a later invocation that genuinely has the env vars set would be
    // silently suppressed. (Pure polish today since env vars are constant
    // within a process, but the name promises "warn-once" not
    // "called-once.")
    if crate::log_level() == Some(0) {
        return;
    }
    let in_use = paths::legacy_otel_env_vars_in_use();
    if in_use.is_empty() {
        return;
    }
    if LEGACY_OTEL_WARNING_EMITTED.set(()).is_err() {
        return;
    }
    eprintln!("{}", legacy_otel_warning(&in_use));
}

fn legacy_otel_warning(in_use: &[&str]) -> String {
    format!(
        "warning: ccusage no longer reads OpenTelemetry exports; the following \
         environment variable{plural} {verb} ignored: {names}. ccusage now reads \
         ${{COPILOT_CONFIG_DIR:-~/.copilot}}/session-state/<sessionId>/events.jsonl. \
         Unset these to silence this warning. See \
         https://ccusage.com/guide/copilot/#data-source",
        plural = if in_use.len() == 1 { "" } else { "s" },
        verb = if in_use.len() == 1 { "is" } else { "are" },
        names = in_use.join(", "),
    )
}

fn empty_usage_message() -> &'static str {
    "No GitHub Copilot CLI usage data found.\nThe Copilot CLI writes per-session events to ~/.copilot/session-state/<uuid>/events.jsonl; verify Copilot CLI is installed and has at least one shutdown record. Override the base directory with COPILOT_CONFIG_DIR if your install lives elsewhere.\nSee https://ccusage.com/guide/copilot/#data-source"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_usage_message_links_to_copilot_docs() {
        let message = empty_usage_message();
        assert!(message.contains("https://ccusage.com/guide/copilot/#data-source"));
    }

    #[test]
    fn empty_usage_message_recommends_session_state() {
        let message = empty_usage_message();
        assert!(
            message.contains("session-state"),
            "session-state must be mentioned: {message}"
        );
        assert!(
            !message.contains("OTel") && !message.contains("OpenTelemetry"),
            "OTel must no longer be recommended in the empty-state message: {message}"
        );
    }

    #[test]
    fn legacy_otel_warning_describes_configurable_session_state_path() {
        let warning = legacy_otel_warning(&["COPILOT_OTEL_DEDUP"]);
        assert!(
            warning.contains(
                "${COPILOT_CONFIG_DIR:-~/.copilot}/session-state/<sessionId>/events.jsonl"
            )
        );
    }
}
