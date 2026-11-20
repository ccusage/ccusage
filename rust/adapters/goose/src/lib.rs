use ccusage_adapter_common::source::{EntryDescriptor, EntrySource, LoadContext};
use ccusage_core::{
    LoadedEntry, Result, TimestampMs, TokenUsageRaw, cli, cli::AgentCommandArgs, date_at,
    parse_ts_timestamp,
};

mod loader;
mod parser;
mod paths;
#[cfg(test)]
mod report;

pub struct Goose;

pub static SOURCE: Goose = Goose;

impl EntrySource for Goose {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor {
            table: ccusage_adapter_common::source::TableStyle::Credits,
            ..EntryDescriptor::standard("goose", "Goose")
        }
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
        loader::load_entries_with_context(cx)
    }
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}
