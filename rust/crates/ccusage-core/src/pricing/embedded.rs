use std::sync::OnceLock;

use serde::Deserialize;

/// Compressed pricing snapshots supplied by the binary at startup.
#[derive(Clone, Copy, Debug)]
pub struct EmbeddedSnapshots {
    pub litellm_sources: &'static [u8],
    pub litellm_pricing: &'static [u8],
    pub models_dev_pricing: &'static [u8],
    pub models_dev_catalog_rules: &'static [u8],
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

fn inflate_snapshot(cell: &'static OnceLock<String>, deflated: &[u8]) -> &'static str {
    cell.get_or_init(|| {
        let bytes = miniz_oxide::inflate::decompress_to_vec(deflated)
            .expect("inflate embedded pricing snapshot");
        String::from_utf8(bytes).expect("embedded pricing snapshot is UTF-8")
    })
}

pub(super) fn build_time_pricing_json() -> &'static str {
    static JSON: OnceLock<String> = OnceLock::new();
    inflate_snapshot(&JSON, embedded_snapshots().litellm_pricing)
}

pub(super) fn build_time_models_dev_json() -> &'static str {
    static JSON: OnceLock<String> = OnceLock::new();
    inflate_snapshot(&JSON, embedded_snapshots().models_dev_pricing)
}

pub(super) fn models_dev_catalog_rules_json() -> &'static str {
    static JSON: OnceLock<String> = OnceLock::new();
    inflate_snapshot(&JSON, embedded_snapshots().models_dev_catalog_rules)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LiteLlmSources {
    pub(super) direct_providers: Vec<String>,
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
