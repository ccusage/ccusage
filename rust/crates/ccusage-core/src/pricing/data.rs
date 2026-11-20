use super::{CacheDefaults, PricingSchedule, ScheduledRates};

// NOTE(2026-11-17): OpenAI documents no cache-write surcharge through GPT-5.5 at https://developers.openai.com/api/docs/guides/prompt-caching. Use the input rate when those models omit a write rate.
pub(super) const WRITES_AT_INPUT_RATE: CacheDefaults = CacheDefaults {
    write_x: 1.0,
    write_1h_x: 1.0,
    read_x: 0.1,
};

// NOTE(2026-11-17): Anthropic documents 1.25× for five-minute writes, 2× for one-hour writes and a standard 0.1× read rate at https://platform.claude.com/docs/en/build-with-claude/prompt-caching. Use these defaults for missing rates.
pub(super) const WRITES_SURCHARGED: CacheDefaults = CacheDefaults {
    write_x: 1.25,
    write_1h_x: 2.0,
    read_x: 0.1,
};

pub(super) static CACHE_DEFAULTS_BY_SEGMENT_PREFIX: &[(&str, &CacheDefaults)] = &[
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
    ("claude-", &WRITES_SURCHARGED),
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

// NOTE(2026-11-20, #2268): DeepSeek launched deepseek-flash with V4.1 rates at 04:00 UTC on September 10, 2026: https://api-docs.deepseek.com/news/news260910/. Its rates differ from the retired V4 Flash schedule.
const DEEPSEEK_FLASH: PricingSchedule = PricingSchedule {
    cutoff_ms: 1_789_012_800_000,
    peak_weekdays: 1..=5,
    peak_hours: &[1..4, 6..10],
    before: ScheduledRates {
        input: 0.3e-6,
        output: 1.2e-6,
        cache_create: 0.3e-6,
        cache_read: 0.006e-6,
    },
    off_peak: ScheduledRates {
        input: 0.15e-6,
        output: 0.6e-6,
        cache_create: 0.15e-6,
        cache_read: 0.003e-6,
    },
    peak: ScheduledRates {
        input: 0.3e-6,
        output: 1.2e-6,
        cache_create: 0.3e-6,
        cache_read: 0.006e-6,
    },
};

pub(super) static SCHEDULES: &[(&str, PricingSchedule)] = &[
    ("deepseek-flash", DEEPSEEK_FLASH),
    ("deepseek-v4-flash", DEEPSEEK_V4_FLASH),
    ("deepseek-v4-pro", DEEPSEEK_V4_PRO),
];
