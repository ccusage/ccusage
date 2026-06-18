use super::*;
use crate::adapter::claude::paths::{claude_paths, usage_files};
use std::fs;

fn enabled() -> bool {
    std::env::var("CCUSAGE_CORPUS").is_ok()
}

#[test]
fn exclusive_reconciles_and_inclusive_dominates() {
    if !enabled() { return; }
    let paths = claude_paths().unwrap_or_default();
    let files = usage_files(&paths, None);
    for f in files.into_iter().take(50) {
        if f.components().any(|c| c.as_os_str() == "subagents") { continue; }
        let Ok(content) = fs::read(&f) else { continue };
        let recs = super::dedup::dedup(super::record::parse_records(&content));
        let t = super::thread::build_thread(&recs, f.to_string_lossy().into(), false);
        let graph = super::thread::link_subagents(t, vec![]);
        let attr = super::attribute::attribute(&graph);

        for s in &attr.skills {
            assert!(s.inclusive.total() + 1.0 >= s.exclusive.total(), "incl<excl in {:?}", f);
        }
        let billed: f64 = recs.iter().filter_map(|r| r.usage).map(|u|
            (u.input_tokens + u.output_tokens + u.cache_creation_token_count() + u.cache_read_input_tokens) as f64).sum();
        let attributed: f64 = attr.skills.iter().map(|s| s.exclusive.total()).sum::<f64>() + attr.baseline.total();
        assert!(attributed <= billed * 1.001 + 1.0, "over-attributed in {:?}: {} > {}", f, attributed, billed);
    }
}
