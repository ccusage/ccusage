use std::{
    collections::{HashMap, HashSet},
    process::Command,
    thread,
};

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;

use crate::{LoadedEntry, PricingMap, Result, cli::CostMode};

use super::parser;

const THREAD_LIST_PAGE_SIZE: usize = 500;
const MAX_EXPORT_WORKERS: usize = 8;

#[derive(Debug, Default)]
pub(super) struct ServerEntries {
    pub(super) entries: Vec<LoadedEntry>,
    pub(super) failed_exports: usize,
    pub(super) list_truncated: bool,
}

#[derive(Deserialize)]
struct ThreadListItem {
    id: Option<String>,
    updated: Option<String>,
}

pub(super) fn load_entries(
    local_latest_usage: &HashMap<String, crate::TimestampMs>,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: &PricingMap,
    single_thread: bool,
    since: Option<&str>,
) -> Result<ServerEntries> {
    let (remote, list_truncated) = list_threads()?;
    let thread_ids = select_remote_thread_items(remote, local_latest_usage, since, tz);
    let mut loaded = export_threads(&thread_ids, tz, mode, pricing, single_thread)?;
    loaded.list_truncated = list_truncated;
    Ok(loaded)
}

fn list_threads() -> Result<(Vec<ThreadListItem>, bool)> {
    let mut threads = Vec::new();
    let mut seen = HashSet::new();
    let mut offset = 0;
    loop {
        let limit = THREAD_LIST_PAGE_SIZE.to_string();
        let offset_text = offset.to_string();
        let output = run_amp(&[
            "threads",
            "list",
            "--include-archived",
            "--limit",
            &limit,
            "--offset",
            &offset_text,
            "--json",
        ])?;
        let page = serde_json::from_slice::<Vec<ThreadListItem>>(&output)?;
        let raw_page_len = page.len();
        let previous_len = threads.len();
        for item in page {
            if let Some(id) = item.id.as_ref()
                && seen.insert(id.clone())
            {
                threads.push(item);
            }
        }
        if raw_page_len < THREAD_LIST_PAGE_SIZE {
            break;
        }
        if threads.len() == previous_len {
            return Ok((threads, true));
        }
        offset += raw_page_len;
    }
    Ok((threads, false))
}

#[cfg(test)]
fn parse_thread_list(output: &[u8]) -> Result<Vec<String>> {
    parse_thread_list_since(output, None)
}

#[cfg(test)]
fn parse_thread_list_since(output: &[u8], since: Option<&str>) -> Result<Vec<String>> {
    let items = serde_json::from_slice::<Vec<ThreadListItem>>(output)?;
    Ok(items
        .into_iter()
        .filter(|item| {
            since.is_none_or(|since| {
                thread_updated_date(item, None)
                    .as_deref()
                    .map(|date| date >= since)
                    .unwrap_or(true)
            })
        })
        .filter_map(|item| item.id)
        .collect())
}

fn select_remote_thread_items(
    remote: Vec<ThreadListItem>,
    local_latest_usage: &HashMap<String, crate::TimestampMs>,
    since: Option<&str>,
    tz: Option<&JiffTimeZone>,
) -> Vec<String> {
    remote
        .into_iter()
        .filter(|item| {
            since.is_none_or(|since| {
                thread_updated_date(item, tz)
                    .as_deref()
                    .map(|date| date >= since)
                    .unwrap_or(true)
            })
        })
        .filter_map(|item| {
            let updated = item.updated.as_deref().and_then(crate::parse_ts_timestamp);
            let id = item.id?;
            if local_latest_usage
                .get(&id)
                .is_none_or(|local| updated.is_none_or(|updated| updated > *local))
            {
                Some(id)
            } else {
                None
            }
        })
        .collect()
}

fn thread_updated_date(item: &ThreadListItem, tz: Option<&JiffTimeZone>) -> Option<String> {
    let timestamp = crate::parse_ts_timestamp(item.updated.as_deref()?)?;
    Some(crate::format_date_tz(timestamp, tz).replace('-', ""))
}

#[cfg(test)]
fn select_remote_threads_updated(
    output: &[u8],
    local_latest_usage: &HashMap<String, crate::TimestampMs>,
) -> Result<Vec<String>> {
    let items = serde_json::from_slice::<Vec<ThreadListItem>>(output)?;
    Ok(select_remote_thread_items(
        items,
        local_latest_usage,
        None,
        None,
    ))
}

fn export_threads(
    thread_ids: &[String],
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: &PricingMap,
    single_thread: bool,
) -> Result<ServerEntries> {
    let worker_count = if single_thread {
        1
    } else {
        thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .min(MAX_EXPORT_WORKERS)
            .min(thread_ids.len())
    };
    if worker_count <= 1 {
        return collect_exports(
            thread_ids
                .iter()
                .map(|thread_id| export_thread(thread_id, tz, mode, pricing)),
        );
    }

    let chunk_size = thread_ids.len().div_ceil(worker_count);
    thread::scope(|scope| {
        let handles = thread_ids
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|thread_id| export_thread(thread_id, tz, mode, pricing))
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>();
        collect_exports(
            handles
                .into_iter()
                .flat_map(|handle| handle.join().expect("Amp export worker panicked")),
        )
    })
}

fn export_thread(
    thread_id: &str,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: &PricingMap,
) -> Result<Vec<LoadedEntry>> {
    let output = run_amp(&["threads", "export", thread_id])?;
    parser::parse_server_thread(&output, thread_id, tz, mode, Some(pricing))
}

fn collect_exports(
    exports: impl IntoIterator<Item = Result<Vec<LoadedEntry>>>,
) -> Result<ServerEntries> {
    let mut loaded = ServerEntries::default();
    for export in exports {
        match export {
            Ok(mut entries) => loaded.entries.append(&mut entries),
            Err(_) => loaded.failed_exports += 1,
        }
    }
    Ok(loaded)
}

fn run_amp(args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("amp").args(args).output()?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(crate::cli_error(format!(
            "amp {} failed: {}",
            args.join(" "),
            detail.trim()
        )));
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn parses_server_thread_list() {
        let output = br#"[
            {"id":"T-new","updated":"2026-07-20T10:00:00.000Z"},
            {"id":"T-old","updated":"2026-03-31T10:00:00.000Z"},
            {"title":"missing id"}
        ]"#;

        let threads = parse_thread_list(output).unwrap();

        assert_eq!(threads, vec!["T-new", "T-old"]);
    }

    #[test]
    fn exports_only_threads_missing_from_legacy_files() {
        let output = br#"[
            {"id":"T-new","updated":"2026-07-20T10:00:00.000Z"},
            {"id":"T-old","updated":"2026-01-20T10:00:00.000Z"}
        ]"#;
        let local = HashMap::from([(
            "T-old".to_string(),
            crate::parse_ts_timestamp("2026-01-21T10:00:00.000Z").unwrap(),
        )]);

        let selected = select_remote_threads_updated(output, &local).unwrap();

        assert_eq!(selected, vec!["T-new"]);
    }

    #[test]
    fn skips_threads_last_updated_before_since() {
        let output = br#"[
            {"id":"T-current","updated":"2026-07-20T10:00:00.000Z"},
            {"id":"T-stale","updated":"2026-06-29T23:59:59.000Z"}
        ]"#;

        let threads = parse_thread_list_since(output, Some("20260701")).unwrap();

        assert_eq!(threads, vec!["T-current"]);
    }

    #[test]
    fn exports_legacy_thread_continued_after_local_history_stopped() {
        let output = br#"[
            {"id":"T-continued","updated":"2026-07-20T10:00:00.000Z"},
            {"id":"T-complete","updated":"2026-01-20T10:00:00.000Z"}
        ]"#;
        let local = HashMap::from([
            (
                "T-continued".to_string(),
                crate::parse_ts_timestamp("2026-03-31T10:00:00.000Z").unwrap(),
            ),
            (
                "T-complete".to_string(),
                crate::parse_ts_timestamp("2026-01-21T10:00:00.000Z").unwrap(),
            ),
        ]);

        let selected = select_remote_threads_updated(output, &local).unwrap();

        assert_eq!(selected, vec!["T-continued"]);
    }
}
