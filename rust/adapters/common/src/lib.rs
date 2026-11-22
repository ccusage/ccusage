use ccusage_core::{LoadedEntry, ReportWindow, cli::SharedArgs};

mod discovery;
mod parallel;
mod walk;

pub use discovery::{DiscoverFiles, DiscoveredFile, read_discovered_files, read_discovered_paths};
pub use parallel::{Split, chunk_file_indexes_by_size, map_parallel, worker_count};
pub use walk::{
    any_file_matching, collect_files_matching, collect_files_with_extension, dir_entries,
    visit_files_matching, visit_usage_files,
};

pub mod billing;
pub mod claims;
pub mod dedupe;
pub mod json;
pub mod jsonl;
pub(crate) mod pipeline;
pub mod report;
pub mod roots;
pub mod rows;
pub mod source;
#[cfg(feature = "sqlite")]
pub mod sqlite;
#[cfg(test)]
mod test_source;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod timestamps;
pub mod units;

#[cfg(any(test, feature = "testing"))]
pub use pipeline::parity;
pub use units::{SqliteStage, UnitError, read_units, read_units_flat};

pub fn filter_loaded_entries_by_date(entries: &mut Vec<LoadedEntry>, shared: &SharedArgs) {
    pipeline::retain_entries_in(entries, &ReportWindow::from_shared(shared));
}
