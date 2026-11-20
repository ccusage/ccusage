use super::{CacheDefaults, PricingSchedule, ScheduledRates};

// NOTE(2026-11-20): OpenAI charges no cache-write surcharge through GPT-5.5 and reads cached input at 0.1× on current models at https://developers.openai.com/api/docs/guides/prompt-caching. Use these multipliers when a row omits cache rates.
pub(super) const WRITES_AT_INPUT_RATE: CacheDefaults = CacheDefaults {
    write_x: 1.0,
    write_1h_x: 1.0,
    read_x: 0.1,
};

// NOTE(2026-11-20): Anthropic lists 1.25× for five-minute writes, 2× for one-hour writes and 0.1× reads for models without a lower read rate at https://platform.claude.com/docs/en/about-claude/pricing. Use these multipliers when a row omits cache rates.
pub(super) const WRITES_SURCHARGED: CacheDefaults = CacheDefaults {
    write_x: 1.25,
    write_1h_x: 2.0,
    read_x: 0.1,
};

// NOTE(2026-11-20): Anthropic reads cache at 0.05× on Opus 5.5 and Sonnet 5.5 at https://platform.claude.com/docs/en/about-claude/pricing. Use these multipliers when a row omits cache rates.
const ANTHROPIC_5_5: CacheDefaults = CacheDefaults {
    read_x: 0.05,
    ..WRITES_SURCHARGED
};

// NOTE(2026-11-20): Anthropic reads cache at 0.025× on Fable and Mythos 5.1 and 5.2 at https://platform.claude.com/docs/en/about-claude/pricing. Use these multipliers when a row omits cache rates.
const ANTHROPIC_FABLE_MYTHOS_5: CacheDefaults = CacheDefaults {
    read_x: 0.025,
    ..WRITES_SURCHARGED
};

// NOTE(2026-11-20): OpenAI charges 1.25× for 30-minute writes and 2.5× for 12-hour writes from GPT-5.6, with 0.1× reads at https://developers.openai.com/api/docs/guides/prompt-caching. Use these multipliers when a row omits cache rates.
const OPENAI_GPT_5_6: CacheDefaults = CacheDefaults {
    write_x: 1.25,
    write_1h_x: 2.5,
    read_x: 0.1,
};

// NOTE(2026-11-20): OpenAI reads cache at 0.05× on GPT-6.2 Astra, GPT-6.2 Sol, GPT-6.1 Sol and GPT-6.1 Terra at https://developers.openai.com/api/docs/guides/prompt-caching. Use these multipliers when a row omits cache rates.
const OPENAI_GPT_6_REDUCED_READ: CacheDefaults = CacheDefaults {
    read_x: 0.05,
    ..OPENAI_GPT_5_6
};

// NOTE(2026-11-20): Gemini 2.5 and 3.x read cache at 0.1× and implicit caching has no write charge; explicit caching adds an hourly storage fee instead at https://ai.google.dev/gemini-api/docs/pricing. Use these multipliers when a row omits cache rates.
const GEMINI: CacheDefaults = CacheDefaults {
    write_x: 1.0,
    write_1h_x: 1.0,
    read_x: 0.1,
};

// NOTE(2026-11-20): Gemini 4 models read cache at 0.05× (e.g. $2.00 input, $0.10 cached) at https://ai.google.dev/gemini-api/docs/pricing. Use these multipliers when a row omits cache rates.
const GEMINI_4: CacheDefaults = CacheDefaults {
    read_x: 0.05,
    ..GEMINI
};

// NOTE(2026-11-20): DeepSeek lists cache hits and misses only, so writes cost the miss rate; V4 Flash hits are $0.006 against $0.30 (0.02×) and V4 Pro 0.033×, so 0.02× is an estimate from the default model at https://api-docs.deepseek.com/quick_start/pricing. Use these multipliers when a row omits cache rates.
const DEEPSEEK: CacheDefaults = CacheDefaults {
    write_x: 1.0,
    write_1h_x: 1.0,
    read_x: 0.02,
};

// NOTE(2026-11-20): xAI lists no cache-write charge and Grok 4.6 and 4.7 read cache at 0.25× ($2.00 input, $0.50 cached) at https://docs.x.ai/developers/pricing. Use these multipliers when a row omits cache rates.
const XAI: CacheDefaults = CacheDefaults {
    write_x: 1.0,
    write_1h_x: 1.0,
    read_x: 0.25,
};

// NOTE(2026-11-20): xAI reads Grok 4.5 cache at 0.15× at https://docs.x.ai/developers/pricing. Use these multipliers when a row omits cache rates.
const XAI_GROK_4_5: CacheDefaults = CacheDefaults {
    read_x: 0.15,
    ..XAI
};

// NOTE(2026-11-20): Moonshot lists Kimi K3 five-minute writes at the input rate, one-hour writes at 2× and cached input at 0.1× at https://platform.moonshot.ai/docs/pricing/chat. Use these multipliers when a row omits cache rates.
const MOONSHOT_KIMI_K3: CacheDefaults = CacheDefaults {
    write_x: 1.0,
    write_1h_x: 2.0,
    read_x: 0.1,
};

// NOTE(2026-11-20): Z.ai lists no cache-write fee and GLM-5, GLM-5.3 and GLM-5.3-Flash cached input at 0.19–0.2× at https://docs.z.ai/guides/overview/pricing. Use these multipliers when a row omits cache rates.
const ZAI_GLM: CacheDefaults = CacheDefaults {
    write_x: 1.0,
    write_1h_x: 1.0,
    read_x: 0.2,
};

// NOTE(2026-11-20): Alibaba Model Studio bills implicit cache creation at 100% and hits at 20% of input; explicit caching (125% / 10%) needs request markers at https://www.alibabacloud.com/help/en/model-studio/context-cache. Use these multipliers when a row omits cache rates.
const QWEN: CacheDefaults = CacheDefaults {
    write_x: 1.0,
    write_1h_x: 1.0,
    read_x: 0.2,
};

// NOTE(2026-11-20): MiniMax bills new input at the standard rate, and M3 reads cache at 0.2× ($0.60 input, $0.12 cached) at https://platform.minimax.io/docs/guides/pricing. Use these multipliers when a row omits cache rates.
const MINIMAX: CacheDefaults = CacheDefaults {
    write_x: 1.0,
    write_1h_x: 1.0,
    read_x: 0.2,
};

// NOTE(2026-11-20): Mistral's own models read cache at 0.1× (e.g. Large 4 $0.7 input, $0.07 cached); it lists no write price, so writes at the input rate are an estimate at https://mistral.ai/pricing/api. Use these multipliers when a row omits cache rates.
const MISTRAL: CacheDefaults = CacheDefaults {
    write_x: 1.0,
    write_1h_x: 1.0,
    read_x: 0.1,
};

// NOTE(2026-11-20): Bedrock reads Amazon Nova cache at 75% below input and lists no Nova cache-write price at https://aws.amazon.com/bedrock/pricing/. Use these multipliers when a row omits cache rates.
const AMAZON_NOVA: CacheDefaults = CacheDefaults {
    write_x: 1.0,
    write_1h_x: 1.0,
    read_x: 0.25,
};

pub(super) static CACHE_DEFAULTS_BY_SEGMENT_PREFIX: &[(&str, &CacheDefaults)] = &[
    ("claude-fable-5-1", &ANTHROPIC_FABLE_MYTHOS_5),
    ("claude-fable-5-2", &ANTHROPIC_FABLE_MYTHOS_5),
    ("claude-mythos-5-1", &ANTHROPIC_FABLE_MYTHOS_5),
    ("claude-mythos-5-2", &ANTHROPIC_FABLE_MYTHOS_5),
    ("claude-opus-5-5", &ANTHROPIC_5_5),
    ("claude-sonnet-5-5", &ANTHROPIC_5_5),
    ("claude-", &WRITES_SURCHARGED),
    ("gpt-6-2-astra", &OPENAI_GPT_6_REDUCED_READ),
    ("gpt-6-2-sol", &OPENAI_GPT_6_REDUCED_READ),
    ("gpt-6-1-sol", &OPENAI_GPT_6_REDUCED_READ),
    ("gpt-6-1-terra", &OPENAI_GPT_6_REDUCED_READ),
    ("openai-gpt-6-2-astra", &OPENAI_GPT_6_REDUCED_READ),
    ("openai-gpt-6-2-sol", &OPENAI_GPT_6_REDUCED_READ),
    ("openai-gpt-6-1-sol", &OPENAI_GPT_6_REDUCED_READ),
    ("openai-gpt-6-1-terra", &OPENAI_GPT_6_REDUCED_READ),
    ("gpt-5-6", &OPENAI_GPT_5_6),
    ("gpt-5-7", &OPENAI_GPT_5_6),
    ("gpt-5-8", &OPENAI_GPT_5_6),
    ("gpt-5-9", &OPENAI_GPT_5_6),
    ("gpt-6", &OPENAI_GPT_5_6),
    ("openai-gpt-5-6", &OPENAI_GPT_5_6),
    ("openai-gpt-5-7", &OPENAI_GPT_5_6),
    ("openai-gpt-5-8", &OPENAI_GPT_5_6),
    ("openai-gpt-5-9", &OPENAI_GPT_5_6),
    ("openai-gpt-6", &OPENAI_GPT_5_6),
    ("codex-", &WRITES_AT_INPUT_RATE),
    ("gpt-", &WRITES_AT_INPUT_RATE),
    ("chatgpt-", &WRITES_AT_INPUT_RATE),
    ("o1-", &WRITES_AT_INPUT_RATE),
    ("o3-", &WRITES_AT_INPUT_RATE),
    ("o4-", &WRITES_AT_INPUT_RATE),
    ("openai-codex-", &WRITES_AT_INPUT_RATE),
    ("openai-gpt-", &WRITES_AT_INPUT_RATE),
    ("openai-chatgpt-", &WRITES_AT_INPUT_RATE),
    ("openai-o1-", &WRITES_AT_INPUT_RATE),
    ("openai-o3-", &WRITES_AT_INPUT_RATE),
    ("openai-o4-", &WRITES_AT_INPUT_RATE),
    ("gemini-4", &GEMINI_4),
    ("gemini-", &GEMINI),
    ("deepseek-", &DEEPSEEK),
    ("grok-4-5", &XAI_GROK_4_5),
    ("grok-", &XAI),
    ("kimi-k3", &MOONSHOT_KIMI_K3),
    ("glm-", &ZAI_GLM),
    ("qwen", &QWEN),
    ("minimax-m", &MINIMAX),
    ("mistral-", &MISTRAL),
    ("devstral-", &MISTRAL),
    ("codestral-", &MISTRAL),
    ("ministral-", &MISTRAL),
    ("magistral-", &MISTRAL),
    ("pixtral-", &MISTRAL),
    ("nova-", &AMAZON_NOVA),
];

pub(super) static CACHE_DEFAULTS_BY_SEGMENT: &[(&str, &CacheDefaults)] = &[
    ("o1", &WRITES_AT_INPUT_RATE),
    ("o3", &WRITES_AT_INPUT_RATE),
    ("o4", &WRITES_AT_INPUT_RATE),
    ("openai-o1", &WRITES_AT_INPUT_RATE),
    ("openai-o3", &WRITES_AT_INPUT_RATE),
    ("openai-o4", &WRITES_AT_INPUT_RATE),
];

pub(super) static CACHE_DEFAULTS_BY_PROVIDER: &[(&str, &CacheDefaults)] = &[
    ("openai", &WRITES_AT_INPUT_RATE),
    ("text-completion-openai", &WRITES_AT_INPUT_RATE),
    ("azure", &WRITES_AT_INPUT_RATE),
    ("azure_text", &WRITES_AT_INPUT_RATE),
    ("anthropic", &WRITES_SURCHARGED),
    ("bedrock", &WRITES_SURCHARGED),
    ("bedrock_converse", &WRITES_SURCHARGED),
    ("vertex_ai-anthropic_models", &WRITES_SURCHARGED),
];

// NOTE(2026-11-19, #2236): A reasoning-effort level selects how a listed model runs, `latest` or `default` selects which snapshot of it answers, and OpenRouter's `:beta` variant routes to the same listing; all bill at that model's rates. Any other word after a listed id names a different model, so a fuzzy match must not borrow the listed id's row for it.
pub(super) static PRICE_NEUTRAL_SUFFIX_WORDS: &[&str] = &[
    "minimal",
    "low",
    "medium",
    "high",
    "xhigh",
    "extra-low",
    "extra-high",
    "latest",
    "default",
    "beta",
];

pub(super) static ALIASES: &[(&str, &str)] = &[
    ("gpt-reserve", "gpt-5.6-luna"),
    ("gpt-5.6", "gpt-5.6-sol"),
    ("gpt-5.3-spark", "gpt-5.3-codex-spark"),
];

// NOTE(2026-08-31, #1679): DeepSeek V4 changed rates on August 16, 2026. Preserve the earlier rates for earlier usage (rates recorded in #1643).
// NOTE(2026-11-17): DeepSeek lists weekday peak windows of 01:00–04:00 and 06:00–10:00 UTC at https://api-docs.deepseek.com/quick_start/pricing/. Use UTC so report timezone cannot change the rate.
const DEEPSEEK_V4_FLASH: PricingSchedule = PricingSchedule {
    cutoff_ms: 1_786_896_000_000,
    peak_weekdays: 1..=5,
    peak_hours: &[1..4, 6..10],
    before: ScheduledRates {
        input: 0.14e-6,
        output: 0.28e-6,
        cache_create: 0.14e-6,
        cache_read: 0.0028e-6,
    },
    off_peak: ScheduledRates {
        input: 0.22e-6,
        output: 0.66e-6,
        cache_create: 0.22e-6,
        cache_read: 0.007e-6,
    },
    peak: ScheduledRates {
        input: 0.44e-6,
        output: 1.32e-6,
        cache_create: 0.44e-6,
        cache_read: 0.014e-6,
    },
};
const DEEPSEEK_V4_PRO: PricingSchedule = PricingSchedule {
    cutoff_ms: 1_786_896_000_000,
    peak_weekdays: 1..=5,
    peak_hours: &[1..4, 6..10],
    before: ScheduledRates {
        input: 0.435e-6,
        output: 0.87e-6,
        cache_create: 0.435e-6,
        cache_read: 0.003625e-6,
    },
    off_peak: ScheduledRates {
        input: 0.66e-6,
        output: 1.98e-6,
        cache_create: 0.66e-6,
        cache_read: 0.022e-6,
    },
    peak: ScheduledRates {
        input: 1.32e-6,
        output: 3.96e-6,
        cache_create: 1.32e-6,
        cache_read: 0.044e-6,
    },
};

pub(super) static SCHEDULES: &[(&str, PricingSchedule)] = &[
    ("deepseek-v4-flash", DEEPSEEK_V4_FLASH),
    ("deepseek-v4-pro", DEEPSEEK_V4_PRO),
];
