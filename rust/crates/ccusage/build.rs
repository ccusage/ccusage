use std::{env, fs};

use serde_json::Value;

const PACKAGE_JSON: &str = "../../../package.json";
const VERSION_ENV: &str = "CCUSAGE_VERSION";

fn main() {
    println!("cargo:rerun-if-env-changed={VERSION_ENV}");
    println!("cargo:rerun-if-changed={PACKAGE_JSON}");
    let version = env::var(VERSION_ENV).unwrap_or_else(|_| {
        let package_json = fs::read_to_string(PACKAGE_JSON).expect("read root package.json");
        serde_json::from_str::<Value>(&package_json)
            .ok()
            .and_then(|package| package.get("version")?.as_str().map(str::to_owned))
            .expect("read version from root package.json")
    });
    println!("cargo:rustc-env={VERSION_ENV}={version}");
}
