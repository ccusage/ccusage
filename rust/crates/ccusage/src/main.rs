mod adapter;
mod cli;
mod http;

use std::process::ExitCode;

use ccusage_cli::Command;
use ccusage_core::{Result, pricing, summary::Bucket};

#[cfg(any(target_os = "macos", all(target_os = "linux", target_env = "musl")))]
#[global_allocator]
static GLOBAL_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn install_embedded_snapshots() {
    pricing::set_embedded_snapshots(pricing::EmbeddedSnapshots {
        litellm_sources: ccusage_pricing_snapshots::LITELLM_SOURCES,
        models_dev_catalog_rules: ccusage_pricing_snapshots::MODELS_DEV_CATALOG_RULES,
        pricing_table: ccusage_pricing_snapshots::PRICING_TABLE,
        models_dev_table: ccusage_pricing_snapshots::MODELS_DEV_TABLE,
    });
}

fn main() -> Result<ExitCode> {
    install_embedded_snapshots();
    pricing::set_json_fetcher(http::fetch_json);
    let command = match cli::parse() {
        Ok(Some(command)) => command,
        Ok(None) => return Ok(ExitCode::SUCCESS),
        Err(status) => return Ok(status),
    };
    match command {
        Command::All(args) => adapter::all::run(args),
        Command::Daily(args) => adapter::claude::run_daily(args),
        Command::Monthly(shared) => adapter::claude::run_bucket(shared, Bucket::Monthly),
        Command::Weekly(args) => {
            adapter::claude::run_bucket(args.shared, Bucket::Weekly(args.start_of_week))
        }
        Command::Session(args) => adapter::claude::run_session(args),
        Command::Blocks(args) => adapter::claude::run_blocks(args),
        Command::Statusline(args) => adapter::claude::run_statusline(args),
        Command::Agent(agent, args) => adapter::all::runner(agent, args),
    }?;
    Ok(ExitCode::SUCCESS)
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
