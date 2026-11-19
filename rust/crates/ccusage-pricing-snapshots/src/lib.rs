//! The pricing snapshots embedded in the ccusage binary.

pub const LITELLM_SOURCES: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/litellm-sources.json.deflate"));

pub const LITELLM_PRICING: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/litellm-pricing.json.deflate"));

pub const MODELS_DEV_PRICING: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/models-dev-pricing.json.deflate"));

/// The committed models.dev catalog rules, applied to both the snapshot and a
/// live models.dev response.
pub const MODELS_DEV_CATALOG_RULES: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/models-dev-catalog-rules.json.deflate"
));

#[cfg(test)]
mod litellm_filter;
