use super::{
    CacheDefaults, LongContext, LongContextMode, LongContextRates, Pricing, Supplement,
    data::{WRITES_AT_INPUT_RATE, WRITES_SURCHARGED, XAI_LONG_CONTEXT_THRESHOLD},
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

// NOTE(2026-11-20): Alibaba bills a whole qwen-plus request above 256K input tokens at $1.2 input and $12 thinking output per 1M at https://www.alibabacloud.com/help/en/model-studio/model-pricing, with the same 125% cache-write and 20% cache-hit ratios at https://www.alibabacloud.com/help/en/model-studio/context-cache. A qwen-plus thinking request above that bills at the tier rates.
const QWEN_PLUS_THINKING: Pricing = Pricing {
    long_context: Some(LongContext {
        threshold: 256_000,
        mode: LongContextMode::WholeRequest,
        rates: LongContextRates {
            input: Some(1.2e-6),
            output: Some(12e-6),
            cache_create: Some(1.5e-6),
            cache_read: Some(0.24e-6),
            cache_create_1h: None,
        },
    }),
    ..rates(0.4e-6, 4e-6, 0.5e-6, 0.08e-6)
};

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
        pricing: rates(3e-6, 15e-6, 3.75e-6, 0.3e-6),
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
    // NOTE(2026-11-25, #2545): xAI lists Grok 4.7 Fast, sold only in Cursor and Grok Build and absent from both catalogues, at $4 input, $1 cached and $12 output per 1M, and $6 / $1.50 / $18 for every token of a request above 200K, at https://docs.x.ai/developers/pricing. xAI bills no cache writes, so a write costs the input rate.
    Supplement {
        provider: "xai",
        model: "grok-4.7-fast",
        pricing: Pricing {
            long_context: Some(LongContext {
                threshold: XAI_LONG_CONTEXT_THRESHOLD,
                mode: LongContextMode::WholeRequest,
                rates: LongContextRates {
                    input: Some(6e-6),
                    output: Some(18e-6),
                    cache_create: Some(6e-6),
                    cache_read: Some(1.5e-6),
                    cache_create_1h: None,
                },
            }),
            ..rates(4e-6, 12e-6, 4e-6, 1e-6)
        },
        context_limit: Some(500000),
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
    // NOTE(2026-11-20): Alibaba lists a separate thinking-mode output price for qwen-plus at https://www.alibabacloud.com/help/en/model-studio/model-pricing ($0.4 input, $1.2 non-thinking and $4 thinking output) and for qwen-turbo ($0.05 / $0.2 / $0.5), qwen3-30b-a3b ($0.2 / $0.8 / $2.4), qwen3-14b ($0.35 / $1.4 / $4.2) and qwen3-8b ($0.18 / $0.7 / $2.1) at https://web.archive.org/web/20260802131610/https://www.alibabacloud.com/help/en/model-studio/model-pricing, all per 1M tokens in the lowest International tier. A thinking id bills at that rate.
    // NOTE(2026-11-20): Alibaba bills an explicit-cache write at 125% of input and an implicit-cache hit, which every qwen-plus request gets without opting in, at 20% of input at https://www.alibabacloud.com/help/en/model-studio/context-cache. qwen-turbo and the Qwen3 open models are not on that page's supported list, so their cache rates are the default ratios and apply only if a host reports cached tokens.
    Supplement {
        provider: "dashscope",
        model: "qwen-plus-thinking",
        pricing: QWEN_PLUS_THINKING,
        context_limit: None,
    },
    // NOTE(2026-11-20): Alibaba lists qwen-plus-latest and qwen-plus-2025-12-01 at https://www.alibabacloud.com/help/en/model-studio/model-pricing, and qwen-plus-2025-09-11 and qwen-plus-2025-07-28 at https://web.archive.org/web/20260802131610/https://www.alibabacloud.com/help/en/model-studio/model-pricing, at the same $0.4 / $1.2 / $4 rates and 256K tier as qwen-plus. Both pages list qwen-plus-2025-07-14 and qwen-plus-2025-04-28 at $0.4 / $1.2 / $4 with "No tiered pricing", and qwen-plus-2025-01-25 with no thinking price.
    Supplement {
        provider: "dashscope",
        model: "qwen-plus-latest-thinking",
        pricing: QWEN_PLUS_THINKING,
        context_limit: None,
    },
    Supplement {
        provider: "dashscope",
        model: "qwen-plus-2025-12-01-thinking",
        pricing: QWEN_PLUS_THINKING,
        context_limit: None,
    },
    Supplement {
        provider: "dashscope",
        model: "qwen-plus-2025-09-11-thinking",
        pricing: QWEN_PLUS_THINKING,
        context_limit: None,
    },
    Supplement {
        provider: "dashscope",
        model: "qwen-plus-2025-07-28-thinking",
        pricing: QWEN_PLUS_THINKING,
        context_limit: None,
    },
    Supplement {
        provider: "dashscope",
        model: "qwen-plus-2025-07-14-thinking",
        pricing: Pricing {
            long_context: None,
            ..QWEN_PLUS_THINKING
        },
        context_limit: None,
    },
    Supplement {
        provider: "dashscope",
        model: "qwen-plus-2025-04-28-thinking",
        pricing: Pricing {
            long_context: None,
            ..QWEN_PLUS_THINKING
        },
        context_limit: None,
    },
    Supplement {
        provider: "dashscope",
        model: "qwen-turbo-thinking",
        pricing: rates(0.05e-6, 0.5e-6, 0.0625e-6, 0.005e-6),
        context_limit: None,
    },
    Supplement {
        provider: "dashscope",
        model: "qwen3-30b-a3b-thinking",
        pricing: rates(0.2e-6, 2.4e-6, 0.25e-6, 0.02e-6),
        context_limit: None,
    },
    Supplement {
        provider: "dashscope",
        model: "qwen3-14b-thinking",
        pricing: rates(0.35e-6, 4.2e-6, 0.4375e-6, 0.035e-6),
        context_limit: None,
    },
    Supplement {
        provider: "dashscope",
        model: "qwen3-8b-thinking",
        pricing: rates(0.18e-6, 2.1e-6, 0.225e-6, 0.018e-6),
        context_limit: None,
    },
];

pub(super) const CURATED_LIST_PRICES: &[Supplement] = &[
    // NOTE(2026-11-21, #2231): Moonshot listed kimi-k2-thinking at $0.60 input (cache miss), $0.15 cache hit and $2.50 output per 1M, and kimi-k2-turbo-preview at $1.15 / $0.15 / $8.00, in the USD price data of https://web.archive.org/web/20260125004043/https://platform.moonshot.ai/docs/pricing/chat. K2 caching is automatic and bills no write charge, so a write costs the cache-miss rate unless the loaded row publishes its own write rate, an explicit 0 included. Moonshot retired the K2 series in May 2026 (https://platform.kimi.ai/docs/llms-full.txt changelog).
    Supplement {
        provider: "moonshot",
        model: "moonshot/kimi-k2-thinking",
        pricing: rates(0.6e-6, 2.5e-6, 0.6e-6, 0.15e-6),
        context_limit: Some(262144),
    },
    Supplement {
        provider: "moonshot",
        model: "moonshot/kimi-k2-turbo-preview",
        pricing: rates(1.15e-6, 8e-6, 1.15e-6, 0.15e-6),
        context_limit: Some(262144),
    },
    // NOTE(2026-11-21, #2231): DeepSeek listed DeepSeek-V3.2 at $0.28 input (cache miss), $0.028 cache hit and $0.42 output per 1M at https://web.archive.org/web/20260101121941/https://api-docs.deepseek.com/quick_start/pricing until V4 replaced it in May 2026. DeepSeek bills no cache writes, so the write rate stays whatever the row publishes, an explicit 0 included, and otherwise the DeepSeek default.
    Supplement {
        provider: "deepseek",
        model: "deepseek/deepseek-v3.2",
        pricing: default_write_rates(0.28e-6, 0.42e-6, 0.028e-6, &WRITES_AT_INPUT_RATE),
        context_limit: None,
    },
    // NOTE(2026-11-21, #2231): Google listed gemini-2.0-flash at $0.10 text, image and video input, $0.40 output and $0.025 context-cache reads per 1M on the paid tier at https://web.archive.org/web/20260101134813/https://ai.google.dev/gemini-api/docs/pricing, and shut it down on June 1, 2026. Vertex AI's $0.15 / $0.60 is a host listing, not Google's API list price.
    Supplement {
        provider: "gemini",
        model: "gemini-2.0-flash",
        pricing: default_write_rates(0.1e-6, 0.4e-6, 0.025e-6, &WRITES_AT_INPUT_RATE),
        context_limit: None,
    },
];
