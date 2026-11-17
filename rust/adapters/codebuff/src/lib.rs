use ccusage_adapter_common::source::{EntryDescriptor, EntrySource, LoadContext};
use ccusage_core::{cli::*, *};

mod loader;
mod parser;
mod paths;

pub struct Codebuff;

pub static SOURCE: Codebuff = Codebuff;

impl EntrySource for Codebuff {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor {
            table: ccusage_adapter_common::source::TableStyle::Credits,
            ..EntryDescriptor::standard("codebuff", "Codebuff")
        }
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
        loader::load_entries_with_context(cx)
    }
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}
