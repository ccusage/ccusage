use std::{borrow::Cow, sync::OnceLock};

use serde::Deserialize;

use crate::fast::FxHashSet;
use crate::pricing::{
    dash_pricing_key_separators, embedded::models_dev_catalog_rules_json, is_pricing_key_separator,
};

use super::ModelsDevModalities;

// NOTE(2026-08-15, #1541): The live models.dev API lists each model under each serving catalogue and omits authorship and authored modalities. The source catalogue supplies this information for rate selection.
#[derive(Debug, Deserialize)]
pub(super) struct ModelsDevCatalogRules {
    owners: FxHashSet<String>,
    platforms: FxHashSet<String>,
    #[serde(rename = "authoredModelIds")]
    authored_model_ids: FxHashSet<String>,
    #[serde(rename = "assetPricedModelIds")]
    asset_priced_model_ids: FxHashSet<String>,
    #[serde(skip)]
    normalized_authored_model_ids: Vec<String>,
}

pub(super) const MODELS_DEV_TRUST_OWNER: u8 = 3;
const MODELS_DEV_TRUST_PLATFORM: u8 = 2;
const MODELS_DEV_TRUST_RESELLER: u8 = 1;

impl ModelsDevCatalogRules {
    pub(super) fn rank(&self, provider_id: &str) -> u8 {
        if self.owners.contains(provider_id) {
            return MODELS_DEV_TRUST_OWNER;
        }
        if self.platforms.contains(provider_id) {
            return MODELS_DEV_TRUST_PLATFORM;
        }
        MODELS_DEV_TRUST_RESELLER
    }

    pub(in crate::pricing) fn is_owner(&self, provider_id: &str) -> bool {
        self.owners.contains(provider_id)
    }

    pub(super) fn is_exact_only(&self, source_model_id: &str, pricing_key: &str) -> bool {
        self.is_tier_variant_of_authored_model(source_model_id)
            || is_unversioned_models_dev_model_id(pricing_key)
    }

    fn is_tier_variant_of_authored_model(&self, source_model_id: &str) -> bool {
        if source_model_id.contains('/') {
            return false;
        }
        let normalized = normalized_models_dev_model_id(source_model_id);
        self.normalized_authored_model_ids.iter().any(|authored| {
            normalized
                .strip_prefix(authored.as_str())
                .is_some_and(|rest| rest.starts_with('-'))
        })
    }

    pub(super) fn is_token_priced(
        &self,
        source_model_id: &str,
        modalities: Option<&ModelsDevModalities>,
    ) -> bool {
        let normalized = normalized_models_dev_model_id(source_model_id);
        if self.asset_priced_model_ids.contains(normalized.as_ref()) {
            return false;
        }
        if self.authored_model_ids.contains(normalized.as_ref()) {
            return true;
        }
        let Some(modalities) = modalities else {
            return true;
        };
        let text_only_output = match modalities.output.as_deref() {
            None => true,
            Some([single]) => single == "text",
            Some(_) => false,
        };
        let accepts_text = match modalities.input.as_deref() {
            None => true,
            Some(input) => input.iter().any(|modality| modality == "text"),
        };
        text_only_output && accepts_text
    }
}

pub(super) fn normalized_models_dev_model_id(model_id: &str) -> Cow<'_, str> {
    if model_id
        .bytes()
        .any(|byte| is_pricing_key_separator(byte) || byte.is_ascii_uppercase())
    {
        Cow::Owned(dash_pricing_key_separators(model_id.to_lowercase().chars()))
    } else {
        Cow::Borrowed(model_id)
    }
}

fn is_unversioned_models_dev_model_id(model_id: &str) -> bool {
    !model_id.bytes().any(|byte| byte.is_ascii_digit())
}

pub(super) fn models_dev_catalog_rules() -> &'static ModelsDevCatalogRules {
    static RULES: OnceLock<ModelsDevCatalogRules> = OnceLock::new();
    RULES.get_or_init(|| {
        let mut rules: ModelsDevCatalogRules =
            serde_json::from_str(models_dev_catalog_rules_json())
                .expect("parse embedded models-dev-catalog-rules.json");
        rules.normalized_authored_model_ids = rules
            .authored_model_ids
            .iter()
            .map(|model_id| normalized_models_dev_model_id(model_id).into_owned())
            .collect();
        rules.normalized_authored_model_ids.sort();
        rules
    })
}
