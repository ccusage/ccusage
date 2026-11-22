//! The pricing snapshots embedded in the ccusage binary.

pub const LITELLM_SOURCES: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/litellm-sources.json.deflate"));

/// The offline pricing catalogue, built from the LiteLLM and models.dev
/// snapshots at compile time.
pub const PRICING_TABLE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/pricing-table.bin"));

/// The compressed offline fallback catalogue for models.dev.
pub const MODELS_DEV_TABLE: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/models-dev-table.bin"));

/// The filtered LiteLLM snapshot the pricing table was built from.
pub const LITELLM_PRICING: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/litellm-pricing.json.deflate"));

/// The models.dev snapshot the pricing table was built from.
pub const MODELS_DEV_PRICING: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/models-dev-pricing.json.deflate"));

/// The committed models.dev catalog rules, applied to both the snapshot and a
/// live models.dev response.
pub const MODELS_DEV_CATALOG_RULES: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/models-dev-catalog-rules.json.deflate"
));

#[cfg(test)]
mod float_probe;
#[cfg(test)]
mod litellm_filter;
