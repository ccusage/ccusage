use ccusage_adapter_common::source::{EntryDescriptor, EntrySource, LoadContext};
use ccusage_core::{cli::*, *};

mod loader;
mod parser;
mod paths;
#[cfg(test)]
mod report;

pub struct Hermes;

pub static SOURCE: Hermes = Hermes;

impl EntrySource for Hermes {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor::standard("hermes", "Hermes")
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
        loader::load_entries_with_context(cx)
    }
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}
