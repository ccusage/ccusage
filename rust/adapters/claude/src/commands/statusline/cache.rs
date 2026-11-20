use std::{
    env, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use ccusage_adapter_common::units::read_json;
use ccusage_core::{TimestampMs, format_rfc3339_millis};
use serde::{Deserialize, Serialize};

use super::StatuslineHook;

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct StatuslineCache {
    pub(super) date: String,
    #[serde(rename = "lastOutput")]
    pub(super) last_output: String,
    #[serde(rename = "lastUpdateTime")]
    pub(super) last_update_time: u64,
    #[serde(rename = "transcriptPath")]
    pub(super) transcript_path: String,
    #[serde(rename = "transcriptMtime")]
    pub(super) transcript_mtime: u64,
    #[serde(rename = "isUpdating", default)]
    pub(super) is_updating: bool,
    pub(super) pid: Option<u32>,
}

impl StatuslineCache {
    pub(super) fn completed(
        hook: &StatuslineHook,
        last_output: String,
        transcript_mtime: u64,
        last_update_time: u64,
    ) -> Self {
        Self {
            date: format_cache_date(last_update_time),
            last_output,
            last_update_time,
            transcript_path: hook.transcript_path.clone(),
            transcript_mtime,
            is_updating: false,
            pid: None,
        }
    }

    fn updating(hook: &StatuslineHook, transcript_mtime: u64, previous: Option<&Self>) -> Self {
        let now = now_millis();
        Self {
            date: format_cache_date(now),
            last_output: previous
                .map(|cache| cache.last_output.clone())
                .unwrap_or_default(),
            last_update_time: previous
                .map(|cache| cache.last_update_time)
                .unwrap_or_default(),
            transcript_path: hook.transcript_path.clone(),
            transcript_mtime,
            is_updating: true,
            pid: Some(std::process::id()),
        }
    }
}

pub(super) fn cached_statusline_output(
    cache: &StatuslineCache,
    current_mtime: u64,
    now: u64,
    refresh_interval: u64,
) -> Option<&str> {
    if cache.last_output.is_empty() {
        return None;
    }
    let expired =
        now.saturating_sub(cache.last_update_time) >= refresh_interval.saturating_mul(1000);
    let file_modified = cache.transcript_mtime != current_mtime;
    if expired || file_modified {
        if cache.is_updating && cache.pid.is_some_and(process_is_alive) {
            return Some(cache.last_output.as_str());
        }
        return None;
    }
    Some(cache.last_output.as_str())
}

#[cfg(unix)]
pub(super) fn process_is_alive(pid: u32) -> bool {
    i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .is_some_and(|pid| rustix::process::test_kill_process(pid).is_ok())
}

#[cfg(not(unix))]
pub(super) fn process_is_alive(pid: u32) -> bool {
    pid == std::process::id()
}

pub(super) fn statusline_cache_path(session_id: &str) -> PathBuf {
    env::temp_dir()
        .join("ccusage-semaphore")
        .join(format!("{session_id}.lock"))
}

pub(super) fn read_statusline_cache(path: &Path) -> Option<StatuslineCache> {
    read_json(path).ok()
}

pub(super) fn write_statusline_cache(path: &Path, cache: StatuslineCache) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec(&cache) {
        let _ = fs::write(path, bytes);
    }
}

pub(super) fn mark_statusline_cache_updating(
    path: &Path,
    hook: &StatuslineHook,
    transcript_mtime: u64,
    previous: Option<&StatuslineCache>,
) {
    write_statusline_cache(
        path,
        StatuslineCache::updating(hook, transcript_mtime, previous),
    );
}

pub(super) fn release_statusline_cache(path: &Path) {
    if let Some(mut cache) = read_statusline_cache(path) {
        cache.is_updating = false;
        cache.pid = None;
        write_statusline_cache(path, cache);
    }
}

pub(super) fn transcript_mtime_ms(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
}

pub(super) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn format_cache_date(millis: u64) -> String {
    format_rfc3339_millis(TimestampMs::from_millis(
        i64::try_from(millis).unwrap_or(i64::MAX),
    ))
}
