use super::{
    CacheDefaults, LongContext, LongContextMode, LongContextRates, Pricing, Supplement,
    data::{WRITES_AT_INPUT_RATE, WRITES_SURCHARGED},
    rates,
};

const fn default_write_rates(
    input: f64,
    output: f64,
    cache_read: f64,
    cache_defaults: &'static CacheDefaults,
) -> Pricing {
    Pricing {
        input,
        output,
        cache_read: Some(cache_read),
        cache_defaults,
        ..Pricing::empty()
    }
}

pub(super) const BUILTIN_PRICING: &[Supplement] = &[
    Supplement {
        provider: "anthropic",
        model: "claude-opus-4-5",
        pricing: rates(5e-6, 25e-6, 6.25e-6, 0.5e-6),
        context_limit: Some(200000),
    },
    Supplement {
        provider: "anthropic",
        model: "claude-opus-4-6",
        pricing: rates(5e-6, 25e-6, 6.25e-6, 0.5e-6),
        context_limit: Some(1000000),
    },
    Supplement {
        provider: "anthropic",
        model: "claude-opus-4-7",
        pricing: rates(5e-6, 25e-6, 6.25e-6, 0.5e-6),
        context_limit: Some(1000000),
    },
    Supplement {
        provider: "anthropic",
        model: "claude-opus-4-8",
        pricing: rates(5e-6, 25e-6, 6.25e-6, 0.5e-6),
        context_limit: Some(1000000),
    },
    Supplement {
        provider: "anthropic",
        model: "claude-haiku-4-5",
        pricing: rates(1e-6, 5e-6, 1.25e-6, 0.1e-6),
        context_limit: Some(200000),
    },
    Supplement {
        provider: "anthropic",
        model: "claude-opus-4",
        pricing: rates(15e-6, 75e-6, 18.75e-6, 1.5e-6),
        context_limit: Some(200000),
    },
    Supplement {
        provider: "anthropic",
        model: "claude-sonnet-4-6",
        pricing: rates(3e-6, 15e-6, 3.75e-6, 0.3e-6),
        context_limit: Some(1000000),
    },
    Supplement {
        provider: "anthropic",
        model: "claude-sonnet-4",
        pricing: Pricing {
            long_context: Some(LongContext {
                threshold: 200_000,
                mode: LongContextMode::Marginal,
                rates: LongContextRates {
                    input: Some(6e-6),
                    output: Some(22.5e-6),
                    cache_create: Some(7.5e-6),
                    cache_read: Some(0.6e-6),
                    cache_create_1h: None,
                },
            }),
            ..rates(3e-6, 15e-6, 3.75e-6, 0.3e-6)
        },
        context_limit: Some(200000),
    },
    Supplement {
        provider: "anthropic",
        model: "claude-3-5-haiku",
        pricing: rates(0.8e-6, 4e-6, 1e-6, 0.08e-6),
        context_limit: Some(200000),
    },
    Supplement {
        provider: "anthropic",
        model: "claude-3-5-haiku-20241022",
        pricing: rates(0.8e-6, 4e-6, 1e-6, 0.08e-6),
        context_limit: Some(200000),
    },
    Supplement {
        provider: "anthropic",
        model: "claude-3-opus",
        pricing: rates(15e-6, 75e-6, 18.75e-6, 1.5e-6),
        context_limit: Some(200000),
    },
    Supplement {
        provider: "anthropic",
        model: "claude-3-sonnet",
        pricing: rates(3e-6, 15e-6, 3.75e-6, 0.3e-6),
        context_limit: Some(200000),
    },
    Supplement {
        provider: "anthropic",
        model: "claude-3-haiku",
        pricing: rates(0.25e-6, 1.25e-6, 0.3e-6, 0.03e-6),
        context_limit: Some(200000),
    },
    Supplement {
        provider: "openai",
        model: "gpt-5",
        pricing: default_write_rates(1.25e-6, 10e-6, 0.125e-6, &WRITES_AT_INPUT_RATE),
        context_limit: None,
    },
    Supplement {
        provider: "openai",
        model: "gpt-5.1",
        pricing: default_write_rates(1.25e-6, 10e-6, 0.125e-6, &WRITES_AT_INPUT_RATE),
        context_limit: None,
    },
    Supplement {
        provider: "openai",
        model: "gpt-5.5",
        pricing: default_write_rates(5e-6, 30e-6, 0.5e-6, &WRITES_AT_INPUT_RATE),
        context_limit: Some(1050000),
    },
    Supplement {
        provider: "xai",
        model: "grok-4.3",
        pricing: Pricing {
            cache_read: None,
            ..rates(1.25e-6, 2.5e-6, 1.25e-6, 0.125e-6)
        },
        context_limit: Some(1000000),
    },
    Supplement {
        provider: "moonshot",
        model: "moonshot/kimi-k2.5",
        pricing: default_write_rates(0.6e-6, 3e-6, 0.1e-6, &WRITES_SURCHARGED),
        context_limit: Some(262144),
    },
    Supplement {
        provider: "moonshot",
        model: "moonshot/kimi-k2.6",
        pricing: default_write_rates(0.95e-6, 4e-6, 0.16e-6, &WRITES_SURCHARGED),
        context_limit: Some(262144),
    },
    Supplement {
        provider: "openai",
        model: "gpt-5.3-codex",
        pricing: default_write_rates(1.75e-6, 14e-6, 0.175e-6, &WRITES_AT_INPUT_RATE),
        context_limit: None,
    },
    Supplement {
        provider: "openai",
        model: "gpt-5.4",
        pricing: default_write_rates(2.5e-6, 15e-6, 0.25e-6, &WRITES_AT_INPUT_RATE),
        context_limit: Some(1050000),
    },
    Supplement {
        provider: "openai",
        model: "gpt-5.4-mini",
        pricing: default_write_rates(0.75e-6, 4.5e-6, 0.075e-6, &WRITES_AT_INPUT_RATE),
        context_limit: None,
    },
    Supplement {
        provider: "openai",
        model: "gpt-5.4-nano",
        pricing: default_write_rates(0.2e-6, 1.25e-6, 0.02e-6, &WRITES_AT_INPUT_RATE),
        context_limit: None,
    },
    Supplement {
        provider: "openai",
        model: "gpt-5.6-sol",
        pricing: rates(5e-6, 30e-6, 6.25e-6, 0.5e-6),
        context_limit: Some(1050000),
    },
    Supplement {
        provider: "openai",
        model: "gpt-5.6-terra",
        pricing: rates(2.5e-6, 15e-6, 3.125e-6, 0.25e-6),
        context_limit: Some(1050000),
    },
    Supplement {
        provider: "openai",
        model: "gpt-5.6-luna",
        pricing: rates(1e-6, 6e-6, 1.25e-6, 0.1e-6),
        context_limit: Some(1050000),
    },
    // NOTE(2026-08-15, #1541): Z.AI lists separate GLM cache-read rates and free cache storage at https://docs.z.ai/guides/overview/pricing. Keep explicit zero write rates so absent catalogue fields do not incur a surcharge.
    Supplement {
        provider: "zai",
        model: "glm-4.5",
        pricing: rates(0.6e-6, 2.2e-6, 0.0, 0.11e-6),
        context_limit: None,
    },
    Supplement {
        provider: "zai",
        model: "zai/glm-4.5",
        pricing: rates(0.6e-6, 2.2e-6, 0.0, 0.11e-6),
        context_limit: None,
    },
    Supplement {
        provider: "zai",
        model: "glm-4.6",
        pricing: rates(0.6e-6, 2.2e-6, 0.0, 0.11e-6),
        context_limit: None,
    },
    Supplement {
        provider: "zai",
        model: "glm-4.7",
        pricing: rates(0.6e-6, 2.2e-6, 0.0, 0.11e-6),
        context_limit: None,
    },
    Supplement {
        provider: "zai",
        model: "zai/glm-4.5-x",
        pricing: rates(2.2e-6, 8.9e-6, 0.0, 0.45e-6),
        context_limit: None,
    },
    Supplement {
        provider: "zai",
        model: "zai/glm-4.5-air",
        pricing: rates(0.2e-6, 1.1e-6, 0.0, 0.03e-6),
        context_limit: None,
    },
    Supplement {
        provider: "zai",
        model: "zai/glm-4.5-airx",
        pricing: rates(1.1e-6, 4.5e-6, 0.0, 0.22e-6),
        context_limit: None,
    },
    Supplement {
        provider: "zai",
        model: "zai/glm-4.5v",
        pricing: rates(0.6e-6, 1.8e-6, 0.0, 0.11e-6),
        context_limit: None,
    },
    Supplement {
        provider: "zai",
        model: "zai/glm-4-32b-0414-128k",
        pricing: rates(0.1e-6, 0.1e-6, 0.0, 0.0),
        context_limit: None,
    },
    Supplement {
        provider: "zai",
        model: "zai/glm-4.5-flash",
        pricing: rates(0.0, 0.0, 0.0, 0.0),
        context_limit: None,
    },
    Supplement {
        provider: "zai",
        model: "glm-5",
        pricing: rates(1e-6, 3.2e-6, 0.0, 0.2e-6),
        context_limit: None,
    },
    Supplement {
        provider: "zai",
        model: "glm-5-turbo",
        pricing: rates(1.2e-6, 4e-6, 0.0, 0.24e-6),
        context_limit: None,
    },
    Supplement {
        provider: "zai",
        model: "glm-5.1",
        pricing: rates(1.4e-6, 4.4e-6, 0.0, 0.26e-6),
        context_limit: None,
    },
];
