mod adapter;
mod blocks;
mod cli;
mod commands;
mod http;

pub(crate) use adapter::claude::{
    load_daily_summaries, load_entries, load_entries_since, load_entries_since_with_diagnostics,
};
pub(crate) use blocks::{
    block_json, calculate_burn_rate, filter_blocks_by_date, format_remaining_time,
    identify_session_blocks, print_active_block_detail, print_blocks_table, sort_blocks,
};
pub(crate) use ccusage_core::*;
pub(crate) use ccusage_report::*;
use cli::Command;

#[cfg(all(target_os = "linux", target_env = "musl"))]
#[global_allocator]
static GLOBAL_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> Result<()> {
    pricing::set_embedded_snapshots(pricing::EmbeddedSnapshots {
        litellm_pricing: ccusage_pricing_snapshots::LITELLM_PRICING,
        models_dev_pricing: ccusage_pricing_snapshots::MODELS_DEV_PRICING,
        models_dev_catalog_rules: ccusage_pricing_snapshots::MODELS_DEV_CATALOG_RULES,
    });
    pricing::set_json_fetcher(http::fetch_json);
    match cli::parse() {
        Command::All(args) => adapter::all::run(args),
        Command::Daily(args) => commands::run_daily(args),
        Command::Monthly(shared) => commands::run_bucket(shared, BucketKind::Monthly),
        Command::Weekly(args) => commands::run_weekly(args),
        Command::Session(args) => commands::run_session(args),
        Command::Blocks(args) => commands::run_blocks(args),
        Command::Statusline(args) => commands::run_statusline(args),
        Command::Codex(args) => adapter::codex::run(args),
        Command::OpenCode(args) => adapter::all::run_opencode(args),
        Command::Amp(args) => adapter::amp::run(args),
        Command::Droid(args) => adapter::droid::run(args),
        Command::Codebuff(args) => adapter::codebuff::run(args),
        Command::Hermes(args) => adapter::hermes::run(args),
        Command::Pi(args) => adapter::pi::run(args),
        Command::Goose(args) => adapter::goose::run(args),
        Command::Kilo(args) => adapter::kilo::run(args),
        Command::Qwen(args) => adapter::qwen::run(args),
        Command::Copilot(args) => adapter::copilot::run(args),
        Command::Gemini(args) => adapter::gemini::run(args),
        Command::Antigravity(args) => adapter::antigravity::run(args),
        Command::Kimi(args) => adapter::kimi::run(args),
        Command::OpenClaw(args) => adapter::openclaw::run(args),
        Command::Grok(args) => adapter::grok::run(args),
        Command::ZCode(args) => adapter::zcode::run(args),
        Command::Cline(args) => adapter::cline::run(args),
        Command::Dsh(args) => adapter::dsh::run(args),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn compiled_version_matches_release_package() {
        let package =
            serde_json::from_str::<serde_json::Value>(include_str!("../../../../package.json"))
                .unwrap();

        assert_eq!(
            env!("CCUSAGE_VERSION"),
            package["version"].as_str().unwrap()
        );
    }
}
