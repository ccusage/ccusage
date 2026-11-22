pub(crate) fn install_pricing_snapshots() {
    crate::pricing::set_embedded_snapshots(crate::pricing::EmbeddedSnapshots {
        litellm_sources: ccusage_pricing_snapshots::LITELLM_SOURCES,
        models_dev_catalog_rules: ccusage_pricing_snapshots::MODELS_DEV_CATALOG_RULES,
        pricing_table: ccusage_pricing_snapshots::PRICING_TABLE,
        models_dev_table: ccusage_pricing_snapshots::MODELS_DEV_TABLE,
    });
}
