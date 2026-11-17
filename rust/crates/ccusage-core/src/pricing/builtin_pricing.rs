use super::{Pricing, ProviderFamily, Supplement, rates};

const fn default_write_rates(
    input: f64,
    output: f64,
    cache_read: f64,
    family: ProviderFamily,
) -> Pricing {
    Pricing {
        input,
        output,
        cache_read: Some(cache_read),
        family,
        ..Pricing::empty()
    }
}

pub(super) const BUILTIN_PRICING: &[Supplement] = &[
    Supplement {
        model: "claude-opus-4-5",
        pricing: rates(5e-6, 25e-6, 6.25e-6, 0.5e-6),
        context_limit: Some(200000),
    },
    Supplement {
        model: "claude-opus-4-6",
        pricing: rates(5e-6, 25e-6, 6.25e-6, 0.5e-6),
        context_limit: Some(1000000),
    },
    Supplement {
        model: "claude-opus-4-7",
        pricing: rates(5e-6, 25e-6, 6.25e-6, 0.5e-6),
        context_limit: Some(1000000),
    },
    Supplement {
        model: "claude-opus-4-8",
        pricing: rates(5e-6, 25e-6, 6.25e-6, 0.5e-6),
        context_limit: Some(1000000),
    },
    Supplement {
        model: "claude-haiku-4-5",
        pricing: rates(1e-6, 5e-6, 1.25e-6, 0.1e-6),
        context_limit: Some(200000),
    },
    Supplement {
        model: "claude-opus-4",
        pricing: rates(15e-6, 75e-6, 18.75e-6, 1.5e-6),
        context_limit: Some(200000),
    },
    Supplement {
        model: "claude-sonnet-4-6",
        pricing: rates(3e-6, 15e-6, 3.75e-6, 0.3e-6),
        context_limit: Some(1000000),
    },
    Supplement {
        model: "claude-sonnet-4",
        pricing: Pricing {
            input_above_200k: Some(6e-6),
            output_above_200k: Some(22.5e-6),
            cache_create_above_200k: Some(7.5e-6),
            cache_read_above_200k: Some(0.6e-6),
            ..rates(3e-6, 15e-6, 3.75e-6, 0.3e-6)
        },
        context_limit: Some(200000),
    },
    Supplement {
        model: "claude-3-5-haiku",
        pricing: rates(0.8e-6, 4e-6, 1e-6, 0.08e-6),
        context_limit: Some(200000),
    },
    Supplement {
        model: "claude-3-5-haiku-20241022",
        pricing: rates(0.8e-6, 4e-6, 1e-6, 0.08e-6),
        context_limit: Some(200000),
    },
    Supplement {
        model: "claude-3-opus",
        pricing: rates(15e-6, 75e-6, 18.75e-6, 1.5e-6),
        context_limit: Some(200000),
    },
    Supplement {
        model: "claude-3-sonnet",
        pricing: rates(3e-6, 15e-6, 3.75e-6, 0.3e-6),
        context_limit: Some(200000),
    },
    Supplement {
        model: "claude-3-haiku",
        pricing: rates(0.25e-6, 1.25e-6, 0.3e-6, 0.03e-6),
        context_limit: Some(200000),
    },
    Supplement {
        model: "gpt-5",
        pricing: default_write_rates(1.25e-6, 10e-6, 0.125e-6, ProviderFamily::OpenAi),
        context_limit: None,
    },
    Supplement {
        model: "gpt-5.1",
        pricing: default_write_rates(1.25e-6, 10e-6, 0.125e-6, ProviderFamily::OpenAi),
        context_limit: None,
    },
    Supplement {
        model: "gpt-5.5",
        pricing: default_write_rates(5e-6, 30e-6, 0.5e-6, ProviderFamily::OpenAi),
        context_limit: Some(1050000),
    },
    Supplement {
        model: "grok-4.3",
        pricing: Pricing {
            cache_read: None,
            ..rates(1.25e-6, 2.5e-6, 1.25e-6, 0.125e-6)
        },
        context_limit: Some(1000000),
    },
    Supplement {
        model: "moonshot/kimi-k2.5",
        pricing: default_write_rates(0.6e-6, 3e-6, 0.1e-6, ProviderFamily::Unrecorded),
        context_limit: Some(262144),
    },
    Supplement {
        model: "moonshot/kimi-k2.6",
        pricing: default_write_rates(0.95e-6, 4e-6, 0.16e-6, ProviderFamily::Unrecorded),
        context_limit: Some(262144),
    },
    Supplement {
        model: "gpt-5.3-codex",
        pricing: default_write_rates(1.75e-6, 14e-6, 0.175e-6, ProviderFamily::OpenAi),
        context_limit: None,
    },
    Supplement {
        model: "gpt-5.4",
        pricing: default_write_rates(2.5e-6, 15e-6, 0.25e-6, ProviderFamily::OpenAi),
        context_limit: Some(1050000),
    },
    Supplement {
        model: "gpt-5.4-mini",
        pricing: default_write_rates(0.75e-6, 4.5e-6, 0.075e-6, ProviderFamily::OpenAi),
        context_limit: None,
    },
    Supplement {
        model: "gpt-5.4-nano",
        pricing: default_write_rates(0.2e-6, 1.25e-6, 0.02e-6, ProviderFamily::OpenAi),
        context_limit: None,
    },
    Supplement {
        model: "gpt-5.6-sol",
        pricing: rates(5e-6, 30e-6, 6.25e-6, 0.5e-6),
        context_limit: Some(1050000),
    },
    Supplement {
        model: "gpt-5.6-terra",
        pricing: rates(2.5e-6, 15e-6, 3.125e-6, 0.25e-6),
        context_limit: Some(1050000),
    },
    Supplement {
        model: "gpt-5.6-luna",
        pricing: rates(1e-6, 6e-6, 1.25e-6, 0.1e-6),
        context_limit: Some(1050000),
    },
    Supplement {
        model: "glm-4.5",
        pricing: rates(0.6e-6, 2.2e-6, 0.0, 0.11e-6),
        context_limit: None,
    },
    Supplement {
        model: "zai/glm-4.5",
        pricing: rates(0.6e-6, 2.2e-6, 0.0, 0.11e-6),
        context_limit: None,
    },
    Supplement {
        model: "glm-4.6",
        pricing: rates(0.6e-6, 2.2e-6, 0.0, 0.11e-6),
        context_limit: None,
    },
    Supplement {
        model: "glm-4.7",
        pricing: rates(0.6e-6, 2.2e-6, 0.0, 0.11e-6),
        context_limit: None,
    },
    Supplement {
        model: "zai/glm-4.5-x",
        pricing: rates(2.2e-6, 8.9e-6, 0.0, 0.45e-6),
        context_limit: None,
    },
    Supplement {
        model: "zai/glm-4.5-air",
        pricing: rates(0.2e-6, 1.1e-6, 0.0, 0.03e-6),
        context_limit: None,
    },
    Supplement {
        model: "zai/glm-4.5-airx",
        pricing: rates(1.1e-6, 4.5e-6, 0.0, 0.22e-6),
        context_limit: None,
    },
    Supplement {
        model: "zai/glm-4.5v",
        pricing: rates(0.6e-6, 1.8e-6, 0.0, 0.11e-6),
        context_limit: None,
    },
    Supplement {
        model: "zai/glm-4-32b-0414-128k",
        pricing: rates(0.1e-6, 0.1e-6, 0.0, 0.0),
        context_limit: None,
    },
    Supplement {
        model: "zai/glm-4.5-flash",
        pricing: rates(0.0, 0.0, 0.0, 0.0),
        context_limit: None,
    },
    Supplement {
        model: "glm-5",
        pricing: rates(1e-6, 3.2e-6, 0.0, 0.2e-6),
        context_limit: None,
    },
    Supplement {
        model: "glm-5-turbo",
        pricing: rates(1.2e-6, 4e-6, 0.0, 0.24e-6),
        context_limit: None,
    },
    Supplement {
        model: "glm-5.1",
        pricing: rates(1.4e-6, 4.4e-6, 0.0, 0.26e-6),
        context_limit: None,
    },
];
