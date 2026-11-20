use ccusage_adapter_common::{source::LoadContext, units::read_typed_units_flat};

use crate::{LoadedEntry, Result};

use super::{
    parser::{event_to_loaded, parse_json_file, parse_jsonl_file},
    paths::discover_log_files,
};

pub(crate) fn load_entries(cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
    let files = discover_log_files(cx.diagnostics)?;
    let pricing = cx.pricing.table(cx.shared.mode);
    let mut events = read_typed_units_flat(
        &files,
        cx.shared.single_thread,
        cx.diagnostics,
        |file| match file.extension().and_then(|extension| extension.to_str()) {
            Some("jsonl") => parse_jsonl_file(file, cx.diagnostics),
            _ => parse_json_file(file, cx.diagnostics),
        },
    );
    events.sort_by_key(|event| event.timestamp);
    Ok(events
        .into_iter()
        .map(|event| event_to_loaded(event, cx.window.timezone(), pricing))
        .collect())
}

#[cfg(test)]
mod tests {
    use ccusage_core::{PricingMap, cli::SharedArgs};
    use ccusage_test_support::fs_fixture;

    #[test]
    fn loads_jsonl_token_events_and_separates_cached_input() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "project/chats/session-a.jsonl": [
                r#"{"sessionId":"session-a","projectHash":"project-a","startTime":"2026-05-17T11:07:00.000Z"}"#,
                r#"{"id":"msg-a","timestamp":"2026-05-17T11:07:32.000Z","type":"gemini","model":"gemini-3-flash-preview","tokens":{"input":15327,"output":23,"cached":11526,"thoughts":919,"tool":7,"total":16276}}"#,
            ]
            .join("\n"),
        });
        ccusage_test_support::Environment::with_var(
            crate::paths::GEMINI_DATA_DIR_ENV,
            fixture.root(),
        )
        .run(|| {
            let shared = SharedArgs {
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                ..SharedArgs::default()
            };
            let entries = ccusage_adapter_common::testing::entries(
                &crate::SOURCE,
                &shared,
                Default::default(),
                Some(&PricingMap::load_embedded()),
            )
            .unwrap();

            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].date.to_string(), "2026-05-17");
            assert_eq!(entries[0].session_id.as_ref(), "session-a");
            assert_eq!(entries[0].model.as_deref(), Some("gemini-3-flash-preview"));
            assert_eq!(entries[0].usage.input_tokens, 3_808);
            assert_eq!(entries[0].usage.output_tokens, 23);
            assert_eq!(entries[0].usage.cache_read_input_tokens, 11_526);
            assert_eq!(entries[0].extra_total_tokens, 919);
        })
    }
}
