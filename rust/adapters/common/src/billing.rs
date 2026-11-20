use ccusage_core::{
    cost::{EntryPrice, EntryPricing, PriceTable, price_entry, price_entry_on_route},
    pricing::BillingRoute,
};

// NOTE(2026-11-20, #2332): Only DeepSeek publishes a time-of-day schedule, so only the provider IDs that bill through DeepSeek itself keep that schedule (https://docs.litellm.ai/docs/providers/deepseek). DeepSeek Harness records its own routes as `deepseek-official` and `deepseek-account`. Every other provider ID bills at a flat rate and must not inherit the schedule, whichever row the lookup reaches.
const DIRECT_PROVIDERS: &[&str] = &["deepseek", "deepseek-official", "deepseek-account"];

const UNKNOWN_PROVIDERS: &[&str] = &["", "unknown"];

/// Billing route for a provider ID recorded by an agent.
///
/// `Direct` means DeepSeek billed the request itself, so its time-of-day
/// schedule applies. `Foreign` means another provider billed it, so the
/// schedule must not. `None` means the record does not name a provider, and the
/// caller keeps the inferred behaviour.
pub fn billing_route(provider: Option<&str>) -> Option<BillingRoute> {
    let provider = provider?.trim().to_ascii_lowercase().replace('_', "-");
    if UNKNOWN_PROVIDERS.contains(&provider.as_str()) {
        return None;
    }
    Some(if DIRECT_PROVIDERS.contains(&provider.as_str()) {
        BillingRoute::Direct
    } else {
        BillingRoute::Foreign
    })
}

/// Prices an entry on the route its recorded provider implies.
///
/// A record with no provider keeps the inferred pricing behaviour.
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
    fn direct_vendor_ids_keep_their_own_schedule() {
        for provider in [
            "deepseek",
            "DeepSeek",
            "deepseek-official",
            "deepseek_account",
        ] {
            assert_eq!(
                billing_route(Some(provider)),
                Some(BillingRoute::Direct),
                "{provider}"
            );
        }
    }

    #[test]
    fn third_party_and_unknown_provider_ids() {
        for provider in [
            "openrouter",
            "bedrock",
            "github-copilot",
            "fireworks_ai",
            "alibaba",
            "anthropic",
            "openai",
            "google",
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
