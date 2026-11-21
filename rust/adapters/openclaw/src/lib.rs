use ccusage_adapter_common::source::{EntryDescriptor, EntrySource, LoadContext};
use ccusage_core::{
    LoadedEntry, Result, TimestampMs, TokenUsageRaw, cli, cli::AgentCommandArgs, date_at, fast,
    model_aliases, parse_ts_timestamp, stored_cost_for_mode,
};

mod loader;
mod parser;
mod paths;
mod sqlite;

pub struct OpenClaw;

pub static SOURCE: OpenClaw = OpenClaw;

impl EntrySource for OpenClaw {
    fn descriptor(&self) -> EntryDescriptor {
        EntryDescriptor {
            session: ccusage_adapter_common::source::SessionShape::WithActivity,
            ..EntryDescriptor::standard("openclaw", "OpenClaw")
        }
    }

    fn load_entries(&self, cx: &LoadContext<'_>) -> Result<Vec<LoadedEntry>> {
        loader::load_entries(cx)
    }
}

pub fn run(args: AgentCommandArgs) -> Result<()> {
    SOURCE.run_focused(args)
}
