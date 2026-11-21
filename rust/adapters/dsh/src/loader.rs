use ccusage_adapter_common::{dedupe::dedupe_highest_rank, read_units, source::LoadContext};

use crate::{LoadedEntry, Result};

use super::{
    parser::{dsh_entry_key, dsh_entry_to_loaded, read_session_file},
    paths::discover_session_files,
};

pub(crate) fn load_entries(cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
    let files = discover_session_files(cx.diagnostics)?;
    let pricing = cx.pricing.table(cx.shared.mode);
    let loaded = read_units(&files, cx.shared.single_thread, cx.diagnostics, |file| {
        read_session_file(file, cx.diagnostics)
    });
    let mut entries = dedupe_highest_rank(
        loaded.into_iter().flatten().flatten(),
        dsh_entry_key,
        |entry| entry.billable_output_tokens(),
    )
    .into_iter()
    .map(|entry| dsh_entry_to_loaded(entry, cx.window.timezone(), pricing))
    .collect::<Vec<_>>();
    LoadedEntry::sort_by_timestamp(&mut entries);
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{Duration, SystemTime},
    };

    use super::super::paths::DSH_HOME_ENV;
    use ccusage_core::{Diagnostics, LoadedEntry, PricingMap, SkipKind, cli::SharedArgs};
    use ccusage_test_support::{Environment, fs_fixture};

    const SESSION: &str = concat!(
        r#"{"type":"session","version":4,"id":"sid-1","createdAt":1785015039278,"cwd":"/home/u/proj"}"#,
        "\n",
        r#"{"type":"request/header","seq":3,"time":1785015040000,"data":{"header":{"config":{"provider":"deepseek-official","model":"deepseek-v4-pro"}},"reason":"initial"}}"#,
        "\n",
        r#"{"type":"assistant/message","seq":5,"time":1785015045000,"data":{"usage":{"inputTokens":1200,"outputTokens":300,"cacheReadTokens":8000,"totalTokens":9500}}}"#,
        "\n",
    );

    const SESSION_FILE: &str = "sessions/--home-u-proj--/sid-1/session.v4.jsonl.zstd";

    fn compressed_session(tail: &[u8]) -> Vec<u8> {
        let (header, events) = SESSION.split_once('\n').unwrap();
        let mut compressed = zstd::stream::encode_all(format!("{header}\n").as_bytes(), 3).unwrap();
        compressed.extend(zstd::stream::encode_all(events.as_bytes(), 3).unwrap());
        compressed.extend(tail);
        compressed
    }

    fn torn_frame() -> Vec<u8> {
        let frame = zstd::stream::encode_all(
            br#"{"type":"assistant/message","seq":9,"time":1785015046000,"data":{"usage":{"inputTokens":9999}}}"#
                .as_slice(),
            3,
        )
        .unwrap();
        frame[..frame.len() / 2].to_vec()
    }

    fn load(fixture: &ccusage_test_support::Fixture) -> (Vec<LoadedEntry>, Diagnostics) {
        Environment::with_var(DSH_HOME_ENV, fixture.root()).run(|| {
            let shared = SharedArgs {
                timezone: Some(ccusage_core::cli::ReportTimezone::UTC),
                ..SharedArgs::default()
            };
            ccusage_adapter_common::testing::load_entries(
                &crate::SOURCE,
                &shared,
                Default::default(),
                Some(&PricingMap::load_embedded()),
            )
            .unwrap()
        })
    }

    fn debug_warnings(diagnostics: &Diagnostics) -> String {
        diagnostics
            .warnings(&SharedArgs {
                debug: true,
                ..SharedArgs::default()
            })
            .join("\n")
    }

    fn age(path: &std::path::Path) {
        let modified = SystemTime::now() - Duration::from_secs(3_600);
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    }

    #[test]
    fn loads_zstd_sessions_split_across_frames_and_skips_a_just_written_torn_tail() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({ "sessions/--home-u-proj--/sid-1/.keep": "" });
        fs::write(
            fixture.path(SESSION_FILE),
            compressed_session(&torn_frame()),
        )
        .unwrap();

        let (entries, diagnostics) = load(&fixture);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date.to_string(), "2026-07-25");
        assert_eq!(entries[0].session_id.as_ref(), "sid-1");
        assert_eq!(entries[0].model.as_deref(), Some("deepseek-v4-pro"));
        assert_eq!(entries[0].usage.input_tokens, 1200);
        assert_eq!(entries[0].usage.cache_read_input_tokens, 8000);
        assert!(entries[0].cost > 0.0);
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        assert!(
            debug_warnings(&diagnostics)
                .contains("zstd stream ends inside a frame; earlier records were kept"),
            "{}",
            debug_warnings(&diagnostics)
        );
    }

    #[test]
    fn skips_a_torn_tail_left_alone_and_keeps_the_records_before_it() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({ "sessions/--home-u-proj--/sid-1/.keep": "" });
        fs::write(
            fixture.path(SESSION_FILE),
            compressed_session(&torn_frame()),
        )
        .unwrap();
        age(&fixture.path(SESSION_FILE));

        let (entries, diagnostics) = load(&fixture);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].usage.input_tokens, 1200);
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
        assert!(
            debug_warnings(&diagnostics)
                .contains("zstd stream ends inside a frame; earlier records were kept"),
            "{}",
            debug_warnings(&diagnostics)
        );
    }

    #[test]
    fn reports_corrupt_zstd_sessions_as_invalid_data() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "sessions/p/not-zstd/session.v4.jsonl.zstd": "not a zstd frame",
            "sessions/--home-u-proj--/sid-1/.keep": "",
        });
        fs::write(
            fixture.path(SESSION_FILE),
            compressed_session(b"not a zstd frame"),
        )
        .unwrap();

        let (entries, diagnostics) = load(&fixture);

        assert_eq!(entries.len(), 1);
        assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 2);
        let warnings = debug_warnings(&diagnostics);
        assert!(warnings.contains("not a valid zstd stream"), "{warnings}");
        assert!(
            warnings.contains("zstd stream is corrupt; earlier records were kept"),
            "{warnings}"
        );
    }

    #[test]
    fn duplicate_dsh_reports_prefer_the_copy_with_recorded_output() {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = PricingMap::default();
        pricing.load_json(r#"{"sample":{"input_cost_per_token":1,"output_cost_per_token":2}}"#);
        let usage = |tokens: &str| {
            format!(
                "{{\"type\":\"session\",\"id\":\"duplicate\",\"createdAt\":1785015039278}}\n\
             {{\"type\":\"request/header\",\"data\":{{\"header\":{{\"config\":{{\"model\":\"sample\"}}}}}}}}\n\
             {{\"type\":\"assistant/message\",\"data\":{{\"usage\":{tokens}}}}}"
            )
        };
        let total_only = usage(r#"{"totalTokens":5}"#);
        let recorded = usage(r#"{"inputTokens":0,"outputTokens":5,"totalTokens":5}"#);
        let first = fs_fixture!({"sessions/p/duplicate/session.v4.jsonl": total_only});
        let second = fs_fixture!({"sessions/p/duplicate/session.v4.jsonl": recorded});
        for [first, second] in [[first.root(), second.root()], [second.root(), first.root()]] {
            let roots = format!("{},{}", first.display(), second.display());
            Environment::with_var(DSH_HOME_ENV, &roots).run(|| {
                let entries = ccusage_adapter_common::testing::entries(
                    &crate::SOURCE,
                    &SharedArgs::default(),
                    Default::default(),
                    Some(&pricing),
                )
                .unwrap();
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].cost, 10.0);
            })
        }
    }

    #[test]
    fn reads_only_the_newest_generation_and_dedupes_across_roots() {
        ccusage_test_support::install_pricing_snapshots();
        let first = fs_fixture!({
            "sessions/p/sid-1/session.v3.jsonl": SESSION,
            "sessions/p/sid-1/session.v4.jsonl": SESSION,
        });
        let second = fs_fixture!({ "sessions/p/sid-1/session.v4.jsonl": SESSION });
        let value = format!("{},{}", first.root().display(), second.root().display());
        Environment::with_var(DSH_HOME_ENV, &value).run(|| {
            let (entries, _) = ccusage_adapter_common::testing::load_entries(
                &crate::SOURCE,
                &SharedArgs::default(),
                Default::default(),
                Some(&PricingMap::load_embedded()),
            )
            .unwrap();

            assert_eq!(entries.len(), 1);
        })
    }
}
