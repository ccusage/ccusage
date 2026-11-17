mod adapter;
mod cli;
mod http;

use ccusage_cli::Command;
use ccusage_core::{BucketKind, Result, pricing};

#[cfg(all(target_os = "linux", target_env = "musl"))]
#[global_allocator]
static GLOBAL_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn install_embedded_snapshots() {
    pricing::set_embedded_snapshots(pricing::EmbeddedSnapshots {
        litellm_pricing: ccusage_pricing_snapshots::LITELLM_PRICING,
        models_dev_pricing: ccusage_pricing_snapshots::MODELS_DEV_PRICING,
        models_dev_catalog_rules: ccusage_pricing_snapshots::MODELS_DEV_CATALOG_RULES,
    });
}

fn main() -> Result<()> {
    install_embedded_snapshots();
    pricing::set_json_fetcher(http::fetch_json);
    match cli::parse() {
        Command::All(args) => adapter::all::run(args),
        Command::Daily(args) => adapter::claude::run_daily(args),
        Command::Monthly(shared) => adapter::claude::run_bucket(shared, BucketKind::Monthly),
        Command::Weekly(args) => adapter::claude::run_weekly(args),
        Command::Session(args) => adapter::claude::run_session(args),
        Command::Blocks(args) => adapter::claude::run_blocks(args),
        Command::Statusline(args) => adapter::claude::run_statusline(args),
        Command::Agent(agent, args) => adapter::all::runner(agent)(args),
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
