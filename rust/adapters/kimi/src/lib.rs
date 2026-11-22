use ccusage_adapter_common::source::{EntryDescriptor, EntrySource, LoadContext};
use ccusage_core::{
    LoadedEntry, Result, TimestampMs, TokenUsageRaw, cli, cli::AgentCommandArgs, date_at, fast,
    model_aliases,
};

mod loader;
mod parser;
mod paths;

pub struct Kimi;

pub static SOURCE: Kimi = Kimi;

impl EntrySource for Kimi {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor {
            ..EntryDescriptor::standard("kimi", "Kimi")
        }
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
        loader::load_entries(cx)
    }
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}
