use crate::{LoadedEntry, Result, cli::SharedArgs};

use super::parser;

pub fn load_entries(shared: &SharedArgs) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(crate::progress::UsageLoadAgent("Qwen"), shared.json, || {
        parser::load_entries(shared)
    })
}
