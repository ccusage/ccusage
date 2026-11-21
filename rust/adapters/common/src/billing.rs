use ccusage_core::{
    cost::{EntryPrice, EntryPricing, PriceTable, price_entry, price_entry_on_route},
    pricing::{BillingRoute, direct_billing_vendor, host_billing_vendor},
};

const UNKNOWN_PROVIDERS: &[&str] = &["", "unknown"];

// NOTE(2026-11-21, #2226): Agents log one host under several names: models.dev's `google-vertex` (https://models.dev/providers/google-vertex), LiteLLM's `vertex_ai` (https://docs.litellm.ai/docs/providers/vertex) and the shorter `vertex`. They all name Vertex AI, a host with its own listing, so they fold to LiteLLM's ID before an adapter builds a `provider/model` candidate or a billing route.
const HOST_ALIASES: &[(&str, &[&str])] = &[(
    "vertex_ai",
    &[
        "vertex",
        "vertex_ai",
        "vertexai",
        "google_vertex",
        "google_vertex_ai",
    ],
)];

/// The catalogue's provider ID for a logged name of a cloud host, such as
/// `vertex_ai` for `google-vertex`, or `None` when the name is not a known
/// host alias. Case, `-` and `_` do not matter.
pub fn host_provider(logged: &str) -> Option<&'static str> {
    let logged = logged.trim().to_ascii_lowercase().replace('-', "_");
    HOST_ALIASES
        .iter()
        .find(|(_, aliases)| aliases.contains(&logged.as_str()))
        .map(|(provider, _)| *provider)
}

/// Billing route for a provider ID recorded by an agent.
///
/// `Direct` names the vendor that billed the request itself, so that vendor's
/// dated schedules apply. `Foreign` means another provider billed it, so no
/// schedule does. `None` means the record does not name a provider, and the
/// caller keeps the inferred behavior.
pub fn billing_route(provider: Option<&str>) -> Option<BillingRoute> {
    let provider = provider?;
    let provider = host_provider(provider)
        .unwrap_or(provider)
        .trim()
        .to_ascii_lowercase()
        .replace('_', "-");
    if UNKNOWN_PROVIDERS.contains(&provider.as_str()) {
        return None;
    }
    Some(
        direct_billing_vendor(&provider)
            .map(BillingRoute::Direct)
            .or_else(|| host_billing_vendor(&provider).map(BillingRoute::Host))
            .unwrap_or(BillingRoute::Foreign),
    )
}

/// Prices an entry on the route its recorded provider implies.
///
/// A record with no provider keeps the inferred pricing behavior.
pub fn price_entry_for_provider<'a>(
    entry: EntryPricing<'a>,
    provider: Option<&str>,
    pricing: PriceTable<'_>,
) -> EntryPrice<'a> {
    match billing_route(provider) {
        Some(route) => price_entry_on_route(entry, route, pricing),
        None => price_entry(entry, pricing),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_vertex_spelling_is_the_vertex_host() {
        for logged in [
            "vertex",
            "vertex_ai",
            "vertex-ai",
            "Vertex-AI",
            "google-vertex",
            "google_vertex",
        ] {
            assert_eq!(host_provider(logged), Some("vertex_ai"), "{logged}");
            assert_eq!(
                billing_route(Some(logged)),
                Some(BillingRoute::Host("google")),
                "{logged}"
            );
        }
        for logged in ["google", "gemini", "openrouter"] {
            assert_eq!(host_provider(logged), None, "{logged}");
        }
    }

    #[test]
    fn direct_vendor_ids_keep_their_own_schedule() {
        for (provider, vendor) in [
            ("deepseek", "deepseek"),
            ("DeepSeek", "deepseek"),
            ("deepseek-official", "deepseek"),
            ("deepseek_account", "deepseek"),
            ("google", "google"),
            ("gemini", "google"),
        ] {
            assert_eq!(
                billing_route(Some(provider)),
                Some(BillingRoute::Direct(vendor)),
                "{provider}"
            );
        }
    }

    #[test]
    fn a_vendors_own_provider_id_names_that_vendor() {
        assert_eq!(
            billing_route(Some("anthropic")),
            Some(BillingRoute::Direct("anthropic"))
        );
        assert_eq!(
            billing_route(Some("Anthropic")),
            Some(BillingRoute::Direct("anthropic"))
        );
        assert_eq!(
            billing_route(Some("xai")),
            Some(BillingRoute::Direct("xai"))
        );
        assert_eq!(
            billing_route(Some("XAI")),
            Some(BillingRoute::Direct("xai"))
        );
    }

    #[test]
    fn third_party_and_unknown_provider_ids() {
        for provider in [
            "openrouter",
            "bedrock",
            "github-copilot",
            "fireworks_ai",
            "alibaba",
            "openai",
        ] {
            assert_eq!(
                billing_route(Some(provider)),
                Some(BillingRoute::Foreign),
                "{provider}"
            );
        }
        for provider in [None, Some(""), Some("  "), Some("unknown")] {
            assert_eq!(billing_route(provider), None, "{provider:?}");
        }
    }
}
