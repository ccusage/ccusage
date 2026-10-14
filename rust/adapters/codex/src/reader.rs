use std::{
    fs::File,
    io::{self, BufRead, BufReader},
    path::{Path, PathBuf},
};

const READ_BUFFER_SIZE: usize = 128 * 1024;

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

pub(super) fn open_rollout_reader(path: &Path) -> io::Result<(Box<dyn BufRead>, PathBuf)> {
    match open_encoded_rollout(path) {
        // Codex may compress or restore the rollout between discovery and this open.
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let other = other_encoding_path(path);
            Ok((open_encoded_rollout(&other)?, other))
        }
        result => Ok((result?, path.to_path_buf())),
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

fn open_encoded_rollout(path: &Path) -> io::Result<Box<dyn BufRead>> {
    let file = File::open(path)?;
    if is_compressed_rollout(path) {
        let decoder = zstd::stream::read::Decoder::new(file)?;
        Ok(Box::new(BufReader::with_capacity(
            READ_BUFFER_SIZE,
            decoder,
        )))
    } else {
        Ok(Box::new(BufReader::with_capacity(READ_BUFFER_SIZE, file)))
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
