use std::sync::OnceLock;

use super::{PricingMap, embedded_models_dev_pricing};

const LITELLM_PRICING_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const MODELS_DEV_API_URL: &str = "https://models.dev/api.json";

impl PricingMap {
    pub(super) fn refresh_litellm<F>(&mut self, fetch_json: F)
    where
        F: FnOnce(&mut dyn FnMut(&str) -> bool) -> std::io::Result<()>,
    {
        let mut loaded_count = 0;
        let fetched = fetch_json(&mut |json| {
            let mut refreshed = Self::default();
            loaded_count = refreshed.load_json(json);
            if loaded_count > 0 {
                refreshed.complete_layers(embedded_models_dev_pricing());
                *self = refreshed;
            }
            loaded_count > 0
        });
        if !should_log_pricing_refresh_details() {
            return;
        }
        match fetched {
            Ok(()) if loaded_count == 0 => {
                ccusage_terminal::errln!(
                    "WARN  Failed to parse LiteLLM pricing; using embedded pricing."
                );
            }
            Ok(()) => {}
            Err(error) => {
                ccusage_terminal::errln!(
                    "WARN  Failed to fetch LiteLLM pricing ({error}); using embedded pricing."
                );
            }
        }
    }
}

fn should_log_pricing_refresh_details() -> bool {
    crate::log_level().is_some_and(|level| level >= 4)
}

pub(super) fn models_dev_pricing() -> Option<&'static PricingMap> {
    static MODELS_DEV_PRICING: OnceLock<Option<PricingMap>> = OnceLock::new();
    load_models_dev_pricing_once(&MODELS_DEV_PRICING, fetch_models_dev_json)
}

pub(super) fn load_models_dev_pricing_once<F>(
    cell: &OnceLock<Option<PricingMap>>,
    fetch_json: F,
) -> Option<&PricingMap>
where
    F: FnOnce(&mut dyn FnMut(&str) -> bool) -> std::io::Result<()>,
{
    cell.get_or_init(|| load_models_dev_pricing(fetch_json))
        .as_ref()
}

pub(super) fn load_models_dev_pricing<F>(fetch_json: F) -> Option<PricingMap>
where
    F: FnOnce(&mut dyn FnMut(&str) -> bool) -> std::io::Result<()>,
{
    let mut map = PricingMap::default();
    let mut loaded = false;
    let fetched = fetch_json(&mut |json| {
        loaded = map
            .load_models_dev_json_missing(json)
            .is_some_and(|loaded_count| loaded_count > 0);
        loaded
    });
    match fetched {
        Ok(()) if loaded => Some(map),
        Ok(()) => {
            if should_log_pricing_refresh_details() {
                ccusage_terminal::errln!(
                    "WARN  Failed to parse models.dev pricing; using LiteLLM pricing."
                );
            }
            None
        }
        Err(error) => {
            if should_log_pricing_refresh_details() {
                ccusage_terminal::errln!(
                    "WARN  Failed to fetch models.dev pricing ({error}); using LiteLLM pricing."
                );
            }
            None
        }
    }
}

pub(super) fn fetch_pricing_json(load: &mut dyn FnMut(&str) -> bool) -> std::io::Result<()> {
    fetch_json_url(LITELLM_PRICING_URL, load)
}

fn fetch_models_dev_json(load: &mut dyn FnMut(&str) -> bool) -> std::io::Result<()> {
    fetch_json_url(MODELS_DEV_API_URL, load)
}

pub type JsonFetcher = fn(&str, &mut dyn FnMut(&str) -> bool) -> std::io::Result<()>;

static JSON_FETCHER: OnceLock<JsonFetcher> = OnceLock::new();

/// Installs the HTTP client used to refresh pricing. Without it, only the embedded snapshots are available.
pub fn set_json_fetcher(fetcher: JsonFetcher) {
    let _ = JSON_FETCHER.set(fetcher);
}

fn fetch_json_url(url: &str, load: &mut dyn FnMut(&str) -> bool) -> std::io::Result<()> {
    let Some(fetch) = JSON_FETCHER.get() else {
        return Err(std::io::Error::other(
            "no HTTP client installed for pricing refresh",
        ));
    };
    fetch(url, load)
}
