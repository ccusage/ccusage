use std::{
    fs::File,
    io::{self, BufReader, Read},
    path::{Path, PathBuf},
};

use ccusage_core::fast::LineReader;

/// Matches zstd's preferred input size, so a whole rollout streams in
/// block-sized reads.
pub(super) const READ_BUFFER_SIZE: usize = 128 * 1024;

type RolloutLines = LineReader<Box<dyn Read>>;

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

/// The `.jsonl` path a rollout has regardless of its encoding.
pub(super) fn uncompressed_rollout_path(path: &Path) -> PathBuf {
    if is_compressed_rollout(path) {
        path.with_extension("")
    } else {
        path.to_path_buf()
    }
}

/// Opens the rollout's lines through windows of `buffer_size` bytes.
pub(super) fn open_rollout_lines(
    path: &Path,
    buffer_size: usize,
) -> io::Result<(RolloutLines, PathBuf)> {
    let (reader, opened) = match open_encoded_rollout(path, buffer_size) {
        // Codex may compress or restore the rollout between discovery and this open.
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let other = other_encoding_path(path);
            (open_encoded_rollout(&other, buffer_size)?, other)
        }
        result => (result?, path.to_path_buf()),
    };
    Ok((LineReader::with_capacity(buffer_size, reader), opened))
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

fn open_encoded_rollout(path: &Path, buffer_size: usize) -> io::Result<Box<dyn Read>> {
    let file = File::open(path)?;
    if is_compressed_rollout(path) {
        Ok(Box::new(zstd::stream::read::Decoder::with_buffer(
            BufReader::with_capacity(buffer_size, file),
        )?))
    } else {
        Ok(Box::new(file))
    }
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
