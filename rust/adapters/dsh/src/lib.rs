use ccusage_adapter_common::source::{EntryDescriptor, EntrySource, LoadContext};
use ccusage_core::{cli::*, *};

mod loader;
mod parser;
mod paths;
#[cfg(test)]
mod report;

#[cfg(test)]
pub(crate) use ccusage_adapter_common::report::plain_report_json as report_from_rows;

pub struct Dsh;

pub static SOURCE: Dsh = Dsh;

impl EntrySource for Dsh {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor {
            ..EntryDescriptor::standard("dsh", "DeepSeek Harness")
        }
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
        loader::load_entries(cx)
    }
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}
