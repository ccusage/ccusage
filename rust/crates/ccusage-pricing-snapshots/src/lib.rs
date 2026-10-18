//! The pricing snapshots embedded in the ccusage binary.
//!
//! `build.rs` minifies and deflates them into `OUT_DIR`; `ccusage-core` inflates
//! them once the binary installs them with
//! `ccusage_core::pricing::set_embedded_snapshots`. Keeping the bytes in this
//! leaf crate, which only the binary depends on, means a routine snapshot
//! refresh recompiles this crate and the binary rather than `ccusage-core` and
//! every adapter built on it.

/// LiteLLM pricing, compacted to the model families ccusage prices offline.
pub const LITELLM_PRICING: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/litellm-pricing.json.deflate"));

/// The committed models.dev pricing snapshot.
pub const MODELS_DEV_PRICING: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/models-dev-pricing.json.deflate"));

/// The committed models.dev catalog rules, applied to both the snapshot and a
/// live models.dev response.
pub const MODELS_DEV_CATALOG_RULES: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/models-dev-catalog-rules.json.deflate"
));

#[cfg(test)]
#[allow(dead_code)]
#[path = "../build.rs"]
mod build_script;
