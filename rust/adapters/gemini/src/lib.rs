use ccusage_adapter_common::source::{EntryDescriptor, EntrySource, LoadContext};
use ccusage_core::{
    LoadedEntry, Result, TimestampMs, TokenUsageRaw, cli, cli::AgentCommandArgs, date_at,
    model_aliases, parse_ts_timestamp,
};

mod loader;
mod parser;
mod paths;

pub struct Gemini;

pub static SOURCE: Gemini = Gemini;

impl EntrySource for Gemini {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor::standard("gemini", "Gemini CLI")
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
        loader::load_entries(cx)
    }
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}
