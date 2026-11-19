use super::{CacheDefaults, Pricing, data};

#[derive(Clone, Copy, Debug)]
pub(super) struct Row {
    pub(super) pricing: Pricing,
    pub(super) context_limit: Option<u64>,
    pub(super) exact_only: bool,
    pub(super) literal_only: bool,
}

impl Default for Row {
    fn default() -> Self {
        Self {
            pricing: Pricing::empty(),
            context_limit: None,
            exact_only: false,
            literal_only: false,
        }
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
