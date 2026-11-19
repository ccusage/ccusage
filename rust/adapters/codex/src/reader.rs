use ccusage_adapter_common::jsonl::{Interruption, ReadPolicy};
use ccusage_core::SkipKind;
use std::path::{Path, PathBuf};

pub(super) const READ_BUFFER_SIZE: usize = 128 * 1024;
// NOTE(2026-11-16, #2102): Codex embeds `base_instructions` in the first `session_meta` line. A 64 KiB probe fits the observed first lines of up to 49.4 KB without growing.
pub(super) const PROBE_BUFFER_SIZE: usize = 64 * 1024;

pub(super) fn is_rollout_file(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "jsonl") || is_compressed_rollout(path)
}

fn is_compressed_rollout(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "zst")
        && path.file_stem().is_some_and(|stem| {
            Path::new(stem)
                .extension()
                .is_some_and(|ext| ext == "jsonl")
        })
}

pub(super) fn uncompressed_rollout_path(path: &Path) -> PathBuf {
    if is_compressed_rollout(path) {
        path.with_extension("")
    } else {
        path.to_path_buf()
    }
}

fn other_encoding_path(path: &Path) -> PathBuf {
    if is_compressed_rollout(path) {
        path.with_extension("")
    } else {
        let mut compressed = path.as_os_str().to_owned();
        compressed.push(".zst");
        PathBuf::from(compressed)
    }
}

pub(super) fn rollout_policy(path: &Path, capacity: usize, keep_records: bool) -> ReadPolicy {
    ReadPolicy {
        capacity,
        compressed: is_compressed_rollout(path),
        // NOTE(2026-10-14, #1890): Codex can compress or restore a rollout after discovery. The other encoding can still hold the same rollout.
        retry_missing: Some(other_encoding_path),
        open_failure: Some("rollout open failed"),
        interrupted: if keep_records {
            |failure| {
                if failure.complete_lines == 0 {
                    return reject_interruption(failure);
                }
                Interruption::KeepRecords(SkipKind::ReadInterrupted, "rollout read failed")
            }
        } else {
            reject_interruption
        },
        ..ReadPolicy::PLAIN
    }
}

fn reject_interruption(failure: ccusage_adapter_common::jsonl::ReadFailure) -> Interruption {
    Interruption::RejectRecords(
        SkipKind::ReadInterrupted,
        "rollout read failed",
        failure.error.into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_plain_and_compressed_rollouts_only() {
        assert!(is_rollout_file(Path::new("a/session.jsonl")));
        assert!(is_rollout_file(Path::new("a/session.jsonl.zst")));
        assert!(!is_rollout_file(Path::new("a/session.zst")));
        assert!(!is_rollout_file(Path::new("a/session.json.zst")));
        assert!(!is_rollout_file(Path::new("a/session.json")));
    }

    #[test]
    fn maps_compressed_rollouts_to_their_plain_path() {
        assert_eq!(
            uncompressed_rollout_path(Path::new("a/session.jsonl.zst")),
            PathBuf::from("a/session.jsonl")
        );
        assert_eq!(
            uncompressed_rollout_path(Path::new("a/session.jsonl")),
            PathBuf::from("a/session.jsonl")
        );
    }
}
