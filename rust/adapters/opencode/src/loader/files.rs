use ccusage_adapter_common::{
    dir_entries, read_units_flat,
    source::UsageLoadContext,
    units::{UnitError, read_filtered_json},
};
use ccusage_core::cost::PriceTable;
use ccusage_core::{Diagnostics, SkipKind};
use jiff::tz::TimeZone as JiffTimeZone;
use std::path::{Path, PathBuf};

use super::{
    DatabaseLoadContext, DateWindow, EntryCollector, LinkedEntry, load_entries_from_database,
};
use crate::{
    LoadedEntry,
    cli::{AgentReportKind, CostMode, SharedArgs},
    collect_files_with_extension,
    parser::{MissingTimestamp, OpenCodeMessage, message_value_to_entry},
};

pub(super) fn load_directories(
    directories: &[PathBuf],
    shared: &SharedArgs,
    report_kind: AgentReportKind,
    diagnostics: &Diagnostics,
) -> Vec<LinkedEntry> {
    let allow_aggregate_fallback = report_kind == AgentReportKind::Session;
    let context = UsageLoadContext::new(shared);
    let tz = context.window().timezone();
    let window = DateWindow::from_shared(shared, tz);
    let mut collector = EntryCollector::default();
    for directory in directories {
        if let Some(db_path) = db_path(directory, diagnostics) {
            load_entries_from_database(
                &DatabaseLoadContext {
                    db_path: &db_path,
                    tz,
                    mode: shared.mode,
                    pricing: context.table(),
                    shared,
                    window,
                    diagnostics,
                },
                allow_aggregate_fallback,
                &mut collector,
            );
        }

        let messages_dir = directory.join("storage").join("message");
        let mut files = Vec::new();
        collect_files_with_extension(&messages_dir, "json", &mut files, diagnostics);

        // NOTE(2026-06-15, #1332): Legacy OpenCode stores messages as storage/message/<sessionID>/<messageID>.json. A claimed file stem lets the loader skip reading the duplicate.
        files.retain(|file| {
            file.file_stem()
                .and_then(|stem| stem.to_str())
                .is_none_or(|stem| !collector.has_claimed(stem))
        });

        let loaded = read_units_flat(&files, shared.single_thread, diagnostics, |file| {
            read_message_file(file, tz, shared.mode, context.table(), window)
        });
        for entry in loaded {
            collector.push_message((entry, None));
        }
    }
    collector.finish()
}

pub(super) fn db_path(opencode_dir: &Path, diagnostics: &Diagnostics) -> Option<PathBuf> {
    let default_path = opencode_dir.join("opencode.db");
    if default_path.is_file() {
        return Some(default_path);
    }
    let mut candidates = dir_entries(opencode_dir, diagnostics)
        .into_iter()
        .map(|(path, _)| path)
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(is_channel_db_name)
        })
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.into_iter().next()
}

fn is_channel_db_name(name: &str) -> bool {
    name.starts_with("opencode-")
        && name.ends_with(".db")
        && name["opencode-".len()..name.len() - ".db".len()]
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn read_message_file(
    path: &Path,
    tz: &JiffTimeZone,
    mode: CostMode,
    pricing: PriceTable<'_>,
    window: DateWindow,
) -> std::result::Result<Option<LoadedEntry>, UnitError> {
    let value =
        read_filtered_json::<OpenCodeMessage>(path, |content| window.admits_payload(content))?;
    let Some(value) = value else {
        return Ok(None);
    };
    message_value_to_entry(value, None, None, tz, mode, pricing).map_err(|MissingTimestamp| {
        UnitError::new(SkipKind::InvalidTimestamp, MissingTimestamp::REASON)
    })
}
