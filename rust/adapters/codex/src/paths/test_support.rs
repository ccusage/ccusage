use std::path::PathBuf;

use super::CodexUsageSource;

impl CodexUsageSource {
    pub(crate) fn new(dir: PathBuf, dedupe_scope: PathBuf) -> Self {
        Self {
            dir,
            dedupe_scope,
            configured_tier: None,
        }
    }
}
