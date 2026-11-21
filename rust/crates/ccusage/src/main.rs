mod adapter;
mod cli;
mod http;

use std::process::ExitCode;

use ccusage_cli::Command;
use ccusage_core::{Result, pricing, summary::Bucket};

#[cfg(all(target_os = "linux", target_env = "musl"))]
#[global_allocator]
static GLOBAL_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

// NOTE(2026-11-20, #2406): mimalloc v3 keeps freed memory committed for 1000 ms, longer than a whole report run. Every block a growing table or vector leaves behind then stays in peak RSS. `MIMALLOC_PURGE_DELAY` still overrides this default.
#[cfg(all(target_os = "linux", target_env = "musl"))]
fn configure_allocator() {
    const MI_OPTION_PURGE_DELAY: libmimalloc_sys::mi_option_t = 15;
    // SAFETY: setting an option default has no preconditions, and no other thread exists yet.
    #[expect(unsafe_code)]
    unsafe {
        libmimalloc_sys::mi_option_set_default(MI_OPTION_PURGE_DELAY, 0);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "musl")))]
fn configure_allocator() {}

fn install_embedded_snapshots() {
    pricing::set_embedded_snapshots(pricing::EmbeddedSnapshots {
        litellm_sources: ccusage_pricing_snapshots::LITELLM_SOURCES,
        models_dev_catalog_rules: ccusage_pricing_snapshots::MODELS_DEV_CATALOG_RULES,
        pricing_table: ccusage_pricing_snapshots::PRICING_TABLE,
    });
}

fn main() -> Result<ExitCode> {
    configure_allocator();
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
