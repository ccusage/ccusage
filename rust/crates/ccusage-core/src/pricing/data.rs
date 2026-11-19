use super::{CacheDefaults, PricingSchedule, ScheduledRates};

pub(super) const WRITES_AT_INPUT_RATE: CacheDefaults = CacheDefaults {
    write_x: 1.0,
    write_1h_x: 1.0,
    read_x: 0.1,
};

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

pub(super) static ALIASES: &[(&str, &str)] = &[
    // NOTE(2026-09-17, #1739): The log label Reserve bills as Luna.
    ("gpt-reserve", "gpt-5.6-luna"),
    // NOTE(2026-07-26, #1438): The unqualified release label bills as Sol.
    ("gpt-5.6", "gpt-5.6-sol"),
    // NOTE(2026-06-12, #1301): The log label omits the catalogue's codex segment.
    ("gpt-5.3-spark", "gpt-5.3-codex-spark"),
];

// NOTE(2026-08-31, #1679): Historical cutoff: https://github.com/ccusage/ccusage/pull/1679; rates: https://github.com/ccusage/ccusage/issues/1643.
// NOTE(2026-11-17): https://api-docs.deepseek.com/quick_start/pricing/ confirms Monday-Friday peak windows at 01:00-04:00 and 06:00-10:00 UTC; preserve the historical schedule.
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
