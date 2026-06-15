mod loader;
mod parser;
mod paths;
mod report;

use crate::{
    PricingMap, Result,
    adapter::opencode,
    cli::{AgentCommandArgs, AgentReportKind},
    filter_loaded_entries_by_date, print_json_or_jq, print_usage_table, sort_summaries, wants_json,
};

pub(crate) use loader::load_entries;
pub(crate) use report::{report_from_rows, summarize_entries};

#[cfg(test)]
struct VibeDataDirEnvGuard {
    _guard: ccusage_test_support::EnvVarGuard,
}

#[cfg(test)]
impl VibeDataDirEnvGuard {
    fn set(path: &std::path::Path) -> Self {
        Self {
            _guard: ccusage_test_support::EnvVarGuard::set(paths::VIBE_DATA_DIR_ENV, path),
        }
    }
}

pub(crate) fn run(args: AgentCommandArgs) -> Result<()> {
    let shared = args.shared;
    let pricing = PricingMap::load_with_overrides(
        shared.offline,
        crate::log_level() != Some(0),
        shared.pricing_overrides.iter(),
    );
    let mut entries = load_entries(&shared)?;
    filter_loaded_entries_by_date(&mut entries, &shared);
    let mut rows = summarize_entries(&entries, args.kind)?;
    sort_summaries(&mut rows, &shared.order, |row| {
        opencode::summary_period(row)
    });
    if wants_json(&shared) {
        return print_json_or_jq(
            report_from_rows(&rows, args.kind),
            shared.jq.as_deref(),
            shared.no_cost,
        );
    }
    print_usage_table(
        "Mistral Vibe Token Usage Report",
        opencode::first_column(args.kind),
        &rows,
        &shared,
        false,
        None,
    )?;
    Ok(())
}

pub(crate) fn has_data() -> bool {
    paths::discover_session_dirs().is_ok_and(|dirs| !dirs.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{AgentReportKind, CostMode, SharedArgs};
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    #[test]
    fn has_data_when_sessions_exist() {
        let fixture = fs_fixture!({
            "session_20260615_172447_abc123/meta.json": "{}",
        });
        let _env_guard = VibeDataDirEnvGuard::set(fixture.root());
        assert!(has_data());
    }

    #[test]
    fn no_data_when_no_sessions() {
        let fixture = fs_fixture!({});
        let _env_guard = VibeDataDirEnvGuard::set(fixture.root());
        assert!(!has_data());
    }
}
