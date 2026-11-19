use super::{CacheDefaults, Pricing, PricingSchedule, data, normalized_pricing_key};

#[derive(Clone, Debug)]
pub(super) struct Row {
    pub(super) pricing: Pricing,
    pub(super) source: Source,
    pub(super) schedule: Option<&'static PricingSchedule>,
    pub(super) context_limit: Option<u64>,
    pub(super) exact_only: bool,
    pub(super) scope: Scope,
    pub(super) vendor_id: Option<Box<str>>,
}

impl Default for Row {
    fn default() -> Self {
        Self {
            pricing: Pricing::empty(),
            source: Source::LiteLlm,
            schedule: None,
            context_limit: None,
            exact_only: false,
            scope: Scope::Hosted,
            vendor_id: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Source {
    Override,
    LiteLlm,
    Curated,
    ModelsDev,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Scope {
    Direct,
    Hosted,
}

impl Row {
    pub(super) fn is_free(&self) -> bool {
        self.source != Source::Override && self.pricing.input == 0.0 && self.pricing.output == 0.0
    }

    pub(super) fn is_bare_hosted(&self, key: &str) -> bool {
        self.scope == Scope::Hosted && !key.contains('/')
    }
}

pub(super) fn vendor_id<'a>(key: &'a str, provider: &str) -> &'a str {
    key.strip_prefix(provider)
        .and_then(|suffix| suffix.strip_prefix('/'))
        .unwrap_or(key)
}

pub(super) fn row_scope(key: &str, provider: &str, direct: &[String]) -> (Scope, Option<Box<str>>) {
    if direct.iter().any(|id| id == provider) {
        (Scope::Direct, Some(vendor_id(key, provider).into()))
    } else {
        (Scope::Hosted, None)
    }
}

pub(super) fn cache_defaults(model: &str, provider: Option<&str>) -> &'static CacheDefaults {
    use data::{
        CACHE_DEFAULTS_BY_PROVIDER, CACHE_DEFAULTS_BY_SEGMENT, CACHE_DEFAULTS_BY_SEGMENT_PREFIX,
    };
    model
        .split(['/', '.'])
        .filter_map(|segment| {
            CACHE_DEFAULTS_BY_SEGMENT_PREFIX
                .iter()
                .find(|(prefix, _)| segment.starts_with(prefix))
                .or_else(|| {
                    CACHE_DEFAULTS_BY_SEGMENT
                        .iter()
                        .find(|(key, _)| segment == *key)
                })
                .map(|(_, defaults)| *defaults)
        })
        .min_by(|left, right| left.write_x.total_cmp(&right.write_x))
        .or_else(|| {
            CACHE_DEFAULTS_BY_PROVIDER
                .iter()
                .find(|(key, _)| Some(*key) == provider)
                .map(|(_, defaults)| *defaults)
        })
        .unwrap_or(&data::WRITES_SURCHARGED)
}

pub(super) fn schedule_for(model: &str) -> Option<&'static PricingSchedule> {
    let identity = normalized_pricing_key(model);
    data::SCHEDULES
        .iter()
        .find_map(|(id, rates)| (*id == identity).then_some(rates))
}
