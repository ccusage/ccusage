use std::sync::OnceLock;

use serde::Deserialize;

use crate::fast::FxHashMap;

use super::{
    Pricing, matches_model_suffix, model_without_date_suffix, normalized_pricing_key, pricing_alias,
};

const FAST_MULTIPLIER_OVERRIDES_JSON: &str = include_str!("../fast-multiplier-overrides.json");
const FLEX_MULTIPLIER_OVERRIDES_JSON: &str = include_str!("../flex-multiplier-overrides.json");
const ULTRAFAST_MULTIPLIER_OVERRIDES_JSON: &str =
    include_str!("../ultrafast-multiplier-overrides.json");

pub(super) struct SpeedMultipliers {
    fast: SpeedMultiplierOverrides,
    ultrafast: SpeedMultiplierOverrides,
    flex: SpeedMultiplierOverrides,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct PublishedSpeed {
    pub(super) fast: Option<f64>,
    pub(super) ultrafast: Option<f64>,
    pub(super) flex: Option<f64>,
}

impl SpeedMultipliers {
    pub(super) fn get() -> &'static Self {
        static TABLES: OnceLock<SpeedMultipliers> = OnceLock::new();
        TABLES.get_or_init(|| Self {
            fast: SpeedMultiplierOverrides::load(FAST_MULTIPLIER_OVERRIDES_JSON),
            ultrafast: SpeedMultiplierOverrides::load(ULTRAFAST_MULTIPLIER_OVERRIDES_JSON),
            flex: SpeedMultiplierOverrides::load(FLEX_MULTIPLIER_OVERRIDES_JSON),
        })
    }

    pub(super) fn for_model(&self, model: &str) -> PublishedSpeed {
        PublishedSpeed {
            fast: self.fast.multiplier_for(model),
            ultrafast: self.ultrafast.multiplier_for(model),
            flex: self.flex.multiplier_for(model),
        }
    }
}

impl PublishedSpeed {
    pub(super) fn or(self, other: Self) -> Self {
        Self {
            fast: self.fast.or(other.fast),
            ultrafast: self.ultrafast.or(other.ultrafast),
            flex: self.flex.or(other.flex),
        }
    }

    pub(super) fn applied_to(self, pricing: Pricing) -> Pricing {
        Pricing {
            fast_multiplier: self.fast.unwrap_or(1.0),
            ultrafast_multiplier: self.ultrafast.unwrap_or(1.0),
            flex_multiplier: self.flex.unwrap_or(1.0),
            ..pricing
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct SpeedMultiplierOverrides {
    exact: FxHashMap<String, f64>,
    normalized_prefix: FxHashMap<String, f64>,
}

impl SpeedMultiplierOverrides {
    fn load(json: &str) -> Self {
        serde_json::from_str(json).expect("parse embedded speed multiplier overrides")
    }

    fn multiplier_for(&self, model: &str) -> Option<f64> {
        if let Some(multiplier) = self.exact.get(model) {
            return Some(*multiplier);
        }
        if let Some(multiplier) = pricing_alias(model).and_then(|alias| self.exact.get(alias)) {
            return Some(*multiplier);
        }
        model
            .split(['/', ':'])
            .flat_map(|part| {
                std::iter::once(part)
                    .chain(part.match_indices('.').map(|(index, _)| &part[index + 1..]))
            })
            .find_map(|part| {
                let base = model_without_date_suffix(part);
                self.exact
                    .get(part)
                    .or_else(|| self.exact.get(base))
                    .copied()
                    .or_else(|| {
                        pricing_alias(base).and_then(|alias| self.exact.get(alias).copied())
                    })
                    .or_else(|| {
                        let normalized = normalized_pricing_key(part);
                        self.normalized_prefix
                            .iter()
                            .find_map(|(base, multiplier)| {
                                matches_model_suffix(&normalized, base).then_some(*multiplier)
                            })
                    })
            })
    }
}
