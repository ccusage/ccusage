use ccusage_adapter_common::source::{EntryDescriptor, EntrySource, LoadContext};
use ccusage_core::{
    LoadedEntry, Result, TimestampMs, TokenUsageRaw, cli, cli::AgentCommandArgs, date_at,
    json_value_u64, parse_ts_timestamp,
};

mod loader;
mod parser;
mod paths;

pub struct Droid;

pub static SOURCE: Droid = Droid;

impl EntrySource for Droid {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor::standard("droid", "Droid")
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
        loader::load_entries_with_context(cx)
    }
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}
