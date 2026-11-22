use ccusage_adapter_common::source::{EntryDescriptor, EntrySource, LoadContext};
use ccusage_core::{
    LoadedEntry, Result, TimestampMs, TokenUsageRaw, cli, cli::AgentCommandArgs, date_at,
    stored_cost_for_mode,
};

mod loader;
mod parser;
mod paths;

pub struct Kilo;

pub static SOURCE: Kilo = Kilo;

impl EntrySource for Kilo {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor::standard("kilo", "Kilo")
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
        loader::load_entries(cx)
    }
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}
