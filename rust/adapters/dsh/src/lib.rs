use ccusage_adapter_common::source::{EntryDescriptor, EntrySource, LoadContext};
use ccusage_core::{
    Diagnostics, LoadedEntry, Result, Skip, SkipKind, TimestampMs, TokenUsageRaw, cli,
    cli::AgentCommandArgs, date_at, fast, total_usage_tokens,
};

mod loader;
mod parser;
mod paths;
#[cfg(test)]
mod report;

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
