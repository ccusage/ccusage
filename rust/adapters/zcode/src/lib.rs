use ccusage_adapter_common::source::{EntryDescriptor, EntrySource, LoadContext};
use ccusage_core::{
    LoadedEntry, Result, TimestampMs, TokenUsageRaw, cli, cli::AgentCommandArgs, date_at,
    debug_log, total_usage_tokens,
};

mod loader;
mod parser;
mod paths;
#[cfg(test)]
mod report;

pub struct Zcode;
pub static SOURCE: Zcode = Zcode;

impl EntrySource for Zcode {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor {
            session: ccusage_adapter_common::source::SessionShape::WithActivity,
            table: ccusage_adapter_common::source::TableStyle::AutoCacheCreation,
            ..EntryDescriptor::standard("zcode", "ZCode")
        }
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
        loader::load_entries_for_context(cx)
    }
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}
