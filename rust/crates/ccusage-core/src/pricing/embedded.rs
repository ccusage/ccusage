use std::sync::OnceLock;

use serde::Deserialize;

use super::{
    PricingMap,
    secondary_index::{INDEX_BYTES, SecondaryIndex},
    table::{EmbeddedRows, RecordOffsets, decode_pricing_table},
};

/// Pricing snapshots supplied by the binary at startup.
#[derive(Clone, Copy, Debug)]
pub struct EmbeddedSnapshots {
    pub litellm_sources: &'static [u8],
    pub models_dev_catalog_rules: &'static [u8],
    /// The offline catalogue built by [`encode_pricing_table`](super::encode_pricing_table).
    pub pricing_table: &'static [u8],
    pub models_dev_table: &'static [u8],
}

static EMBEDDED_SNAPSHOTS: OnceLock<EmbeddedSnapshots> = OnceLock::new();

/// Installs the snapshots before the first pricing lookup. Later calls are ignored.
pub fn set_embedded_snapshots(snapshots: EmbeddedSnapshots) {
    let _ = EMBEDDED_SNAPSHOTS.set(snapshots);
}

fn embedded_snapshots() -> &'static EmbeddedSnapshots {
    EMBEDDED_SNAPSHOTS
        .get()
        .expect("embedded pricing snapshots must be installed with set_embedded_snapshots")
}

fn inflate(deflated: &[u8]) -> Vec<u8> {
    miniz_oxide::inflate::decompress_to_vec(deflated).expect("inflate embedded pricing snapshot")
}

fn inflate_snapshot(cell: &'static OnceLock<String>, deflated: &[u8]) -> &'static str {
    cell.get_or_init(|| {
        String::from_utf8(inflate(deflated)).expect("embedded pricing snapshot is UTF-8")
    })
}

#[cfg(test)]
pub(super) fn build_time_pricing_json() -> &'static str {
    static JSON: OnceLock<String> = OnceLock::new();
    inflate_snapshot(&JSON, ccusage_pricing_snapshots::LITELLM_PRICING)
}

#[cfg(test)]
pub(super) fn build_time_models_dev_json() -> &'static str {
    static JSON: OnceLock<String> = OnceLock::new();
    inflate_snapshot(&JSON, ccusage_pricing_snapshots::MODELS_DEV_PRICING)
}

pub(super) fn embedded_pricing_table() -> &'static PricingMap {
    static TABLE: OnceLock<PricingMap> = OnceLock::new();
    TABLE.get_or_init(|| {
        PricingMap::from_embedded_rows(decode_pricing_table(
            embedded_snapshots().pricing_table,
            RecordOffsets::Absolute,
        ))
    })
}

pub(super) fn embedded_models_dev_rows() -> &'static EmbeddedRows {
    static TABLE: OnceLock<EmbeddedRows> = OnceLock::new();
    TABLE.get_or_init(inflate_models_dev_table)
}

pub(super) fn embedded_models_dev_index() -> SecondaryIndex {
    SecondaryIndex::new(&embedded_snapshots().models_dev_table[..INDEX_BYTES])
}

#[cold]
#[inline(never)]
fn inflate_models_dev_table() -> EmbeddedRows {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    decode_pricing_table(
        BYTES.get_or_init(|| inflate(&embedded_snapshots().models_dev_table[INDEX_BYTES..])),
        RecordOffsets::Delta,
    )
}

pub(super) fn models_dev_catalog_rules_json() -> &'static str {
    static JSON: OnceLock<String> = OnceLock::new();
    inflate_snapshot(&JSON, embedded_snapshots().models_dev_catalog_rules)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LiteLlmSources {
    pub(super) direct_providers: Vec<String>,
    #[serde(default)]
    pub(super) zero_token_rate_unpriced_providers: Vec<String>,
    #[serde(default)]
    pub(super) paid_host_providers: Vec<String>,
}

pub(super) fn litellm_sources() -> Option<&'static LiteLlmSources> {
    static JSON: OnceLock<String> = OnceLock::new();
    static SOURCES: OnceLock<Option<LiteLlmSources>> = OnceLock::new();
    let snapshots = EMBEDDED_SNAPSHOTS.get()?;
    SOURCES
        .get_or_init(|| {
            match serde_json::from_str(inflate_snapshot(&JSON, snapshots.litellm_sources)) {
                Ok(sources) => Some(sources),
                Err(error) => {
                    ccusage_terminal::errln!(
                        "WARN  Failed to parse embedded pricing source rules ({error})."
                    );
                    None
                }
            }
        })
        .as_ref()
}
