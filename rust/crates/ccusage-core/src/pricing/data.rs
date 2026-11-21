use super::{
    CacheDefaults, LongContext, LongContextMode, LongContextRates, PricingSchedule, ScheduledRates,
    Successor,
};

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
pub(super) const OPENAI_GPT_6_REDUCED_READ: CacheDefaults = CacheDefaults {
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

// NOTE(2026-11-20): These vendors bill a thinking or reasoning run as ordinary output tokens at the model's own rates, so the variant word names the same model only for that vendor's ids. Alibaba prices some Qwen models' thinking output separately (https://www.alibabacloud.com/help/en/model-studio/model-pricing), so the word stays a model word for every other family.
pub(super) static FAMILY_PRICE_NEUTRAL_SUFFIX_WORDS: &[(&str, &[&str])] = &[
    // NOTE(2026-11-20): Anthropic bills thinking as output tokens at https://platform.claude.com/docs/en/build-with-claude/thinking-steering-and-cost. A thinking id bills at its model's rates.
    ("claude-", &["thinking"]),
    // NOTE(2026-11-20): Google lists one output price including thinking tokens per Gemini model at https://ai.google.dev/gemini-api/docs/pricing. A thinking id bills at its model's rates.
    ("gemini-", &["thinking"]),
    // NOTE(2026-11-20): DeepSeek lists one price for both thinking and non-thinking modes at https://api-docs.deepseek.com/quick_start/pricing. A thinking id bills at its model's rates.
    ("deepseek-", &["thinking"]),
    // NOTE(2026-11-20): Z.ai turns thinking on per request at https://docs.z.ai/guides/capabilities/thinking-mode and lists one output price per GLM model at https://docs.z.ai/guides/overview/pricing. A thinking id bills at its model's rates.
    ("glm-", &["thinking"]),
    // NOTE(2026-11-20): xAI lists reasoning and non-reasoning ids at identical rates at https://docs.x.ai/developers/pricing. Either id bills at its model's rates.
    ("grok-", &["non-reasoning", "reasoning"]),
    // NOTE(2026-11-20): Moonshot states that `reasoning_content` counts towards the input/output token quota at https://platform.kimi.ai/docs/guide/use-thinking-models.md and lists one output price per Kimi model at https://platform.kimi.ai/docs/pricing/chat. A thinking id bills at its model's rates.
    ("kimi-", &["thinking"]),
    // NOTE(2026-11-20): Mistral defines `reasoning_tokens` as output tokens at https://docs.mistral.ai/studio/agents/reference/turn-records.md, says reasoning only raises output token usage at https://docs.mistral.ai/inference/conversations/reasoning and lists one output price per model at https://docs.mistral.ai/inference/pricing. A thinking id bills at its model's rates.
    ("mistral-", &["thinking"]),
    // NOTE(2026-11-20): Alibaba lists these Qwen models under "Non-thinking and thinking modes" with one output price, or with equal non-thinking and thinking output prices, at https://www.alibabacloud.com/help/en/model-studio/model-pricing (qwen3-max at https://web.archive.org/web/20260802131610/https://www.alibabacloud.com/help/en/model-studio/model-pricing). A thinking id bills at its model's rates. qwen-plus, qwen-turbo and the open qwen3-8b/14b/30b-a3b/235b-a22b models charge more for thinking output, so they are absent here.
    ("qwen4-max", &["thinking"]),
    ("qwen3.8-max", &["thinking"]),
    ("qwen3.7-max", &["thinking"]),
    ("qwen3.6-max", &["thinking"]),
    ("qwen3-max", &["thinking"]),
    ("qwen3.7-plus", &["thinking"]),
    ("qwen3.6-plus", &["thinking"]),
    ("qwen3.5-plus", &["thinking"]),
    ("qwen3.8-flash", &["thinking"]),
    ("qwen3.7-flash", &["thinking"]),
    ("qwen3.6-flash", &["thinking"]),
    ("qwen3.5-flash", &["thinking"]),
    ("qwen-flash", &["thinking"]),
    ("qwen3-vl-plus", &["thinking"]),
    ("qwen3-vl-flash", &["thinking"]),
    ("qwen3.8-27b", &["thinking"]),
    ("qwen3.6-35b-a3b", &["thinking"]),
    ("qwen3.6-27b", &["thinking"]),
    ("qwen3.5-397b-a17b", &["thinking"]),
    ("qwen3.5-122b-a10b", &["thinking"]),
    ("qwen3.5-35b-a3b", &["thinking"]),
    ("qwen3.5-27b", &["thinking"]),
    ("qwen3-32b", &["thinking"]),
];

// NOTE(2026-11-21, #2227): xAI lists separate "(≥ 200k prompt tokens)" rows and bills every token of such a request at them at https://docs.x.ai/developers/pricing. LiteLLM's above-200k fields on xAI's own rows describe that tier, not a marginal one. A request is long when its context exceeds the threshold, so 199,999 makes a 200,000-token prompt long.
const XAI_LONG_CONTEXT_THRESHOLD: u64 = 199_999;
pub(super) static WHOLE_REQUEST_LONG_CONTEXT_PROVIDERS: &[(&str, u64)] =
    &[("xai", XAI_LONG_CONTEXT_THRESHOLD)];

pub(super) struct CuratedLongContext {
    pub(super) input: f64,
    pub(super) output: f64,
    pub(super) long_context: LongContext,
}

const fn whole_request_tier(
    input: f64,
    output: f64,
    threshold: u64,
    tier: [f64; 5],
) -> CuratedLongContext {
    let [
        tier_input,
        tier_output,
        cache_create,
        cache_create_1h,
        cache_read,
    ] = tier;
    CuratedLongContext {
        input,
        output,
        long_context: LongContext {
            threshold,
            mode: LongContextMode::WholeRequest,
            rates: LongContextRates {
                input: Some(tier_input),
                output: Some(tier_output),
                cache_create: Some(cache_create),
                cache_create_1h: Some(cache_create_1h),
                cache_read: Some(cache_read),
            },
        },
    }
}

// NOTE(2026-11-21, #2235): Anthropic billed every token of a Sonnet 4 or Sonnet 4.5 request above 200K input tokens (cache reads and writes included) at $6 input and $22.50 output per 1M, with the prompt caching multipliers on top, at https://web.archive.org/web/20260201050917/https://platform.claude.com/docs/en/about-claude/pricing. The current page no longer lists that tier because both models are back to a 200K window, so the rates still apply to the requests sent while the 1M beta ran. Every row at a model's list rates takes its tier, whichever catalogue it comes from; a host that bills its own rates keeps its own.
const ANTHROPIC_SONNET_4_LONG_CONTEXT: CuratedLongContext =
    whole_request_tier(3e-6, 15e-6, 200_000, [6e-6, 22.5e-6, 7.5e-6, 12e-6, 0.6e-6]);

// NOTE(2026-11-21, #2235): Anthropic bills a whole Haiku 5.5 request whose prompt is over 100,000 tokens at $0.50 input, $0.625 five-minute writes, $1 one-hour writes, $0.05 cache hits and $2.50 output per 1M at https://platform.claude.com/docs/en/about-claude/pricing. Claude 4.6 and later models other than Haiku 5.5 bill their full 1M window at standard rates on the same page.
const ANTHROPIC_HAIKU_5_5_LONG_CONTEXT: CuratedLongContext = whole_request_tier(
    0.1e-6,
    0.5e-6,
    100_000,
    [0.5e-6, 2.5e-6, 0.625e-6, 1e-6, 0.05e-6],
);

pub(super) static ANTHROPIC_LONG_CONTEXT: &[(&str, CuratedLongContext)] = &[
    ("claude-sonnet-4", ANTHROPIC_SONNET_4_LONG_CONTEXT),
    ("claude-sonnet-4-5", ANTHROPIC_SONNET_4_LONG_CONTEXT),
    ("claude-haiku-5-5", ANTHROPIC_HAIKU_5_5_LONG_CONTEXT),
];

pub(super) static ALIASES: &[(&str, &str)] = &[
    ("gpt-reserve", "gpt-5.6-luna"),
    ("gpt-5.6", "gpt-5.6-sol"),
    ("gpt-5.3-spark", "gpt-5.3-codex-spark"),
];

// NOTE(2026-08-31, #1679): DeepSeek V4 changed rates on August 16, 2026. Preserve the earlier rates for earlier usage (rates recorded in #1643).
// NOTE(2026-11-17): DeepSeek lists weekday peak windows of 01:00–04:00 and 06:00–10:00 UTC at https://api-docs.deepseek.com/quick_start/pricing/. Use UTC so report timezone cannot change the rate.
// NOTE(2026-11-20, #2401): DeepSeek's new pricing took effect at 04:00 UTC on September 10, 2026, and the retired V4 Flash ids are billed at the Flash rates from that instant: https://api-docs.deepseek.com/news/news260910/. Legacy ids hand off to the `deepseek-flash` rates from then on.
const DEEPSEEK_V4_1_FLASH_MS: i64 = 1_789_012_800_000;
const DEEPSEEK_V4_FLASH: PricingSchedule = PricingSchedule {
    cutoff_ms: 1_786_896_000_000,
    peak_weekdays: 1..=5,
    peak_hours: &[1..4, 6..10],
    before: ScheduledRates {
        input: 0.14e-6,
        output: 0.28e-6,
        cache_create: 0.14e-6,
        cache_read: 0.0028e-6,
        long_context: None,
    },
    off_peak: ScheduledRates {
        input: 0.22e-6,
        output: 0.66e-6,
        cache_create: 0.22e-6,
        cache_read: 0.007e-6,
        long_context: None,
    },
    peak: ScheduledRates {
        input: 0.44e-6,
        output: 1.32e-6,
        cache_create: 0.44e-6,
        cache_read: 0.014e-6,
        long_context: None,
    },
    // NOTE(2026-11-20, #2401): DeepSeek retired V4 Flash but still accepts `deepseek-v4-flash` and `deepseek-v4-flash-vision-exp` at https://api-docs.deepseek.com/quick_start/pricing/. Both ids follow this handoff instead of the V4 Flash rates above.
    successor: Some(Successor {
        from_ms: DEEPSEEK_V4_1_FLASH_MS,
        schedule: &DEEPSEEK_FLASH,
    }),
    follows_row: false,
    vendor: "deepseek",
};
// NOTE(2026-11-20, #2401): DeepSeek planned to route `deepseek-v4-pro` to V4.1 Flash on September 14, 2026 but kept serving it with unchanged billing instead, and the pricing page still lists its own rates at https://api-docs.deepseek.com/quick_start/pricing/. Give it no successor so it keeps the V4 Pro schedule.
const DEEPSEEK_V4_PRO: PricingSchedule = PricingSchedule {
    cutoff_ms: 1_786_896_000_000,
    peak_weekdays: 1..=5,
    peak_hours: &[1..4, 6..10],
    before: ScheduledRates {
        input: 0.435e-6,
        output: 0.87e-6,
        cache_create: 0.435e-6,
        cache_read: 0.003625e-6,
        long_context: None,
    },
    off_peak: ScheduledRates {
        input: 0.66e-6,
        output: 1.98e-6,
        cache_create: 0.66e-6,
        cache_read: 0.022e-6,
        long_context: None,
    },
    peak: ScheduledRates {
        input: 1.32e-6,
        output: 3.96e-6,
        cache_create: 1.32e-6,
        cache_read: 0.044e-6,
        long_context: None,
    },
    successor: None,
    follows_row: false,
    vendor: "deepseek",
};

// NOTE(2026-11-20, #2268): DeepSeek launched deepseek-flash with V4.1 rates at 04:00 UTC on September 10, 2026: https://api-docs.deepseek.com/news/news260910/. Its rates differ from the retired V4 Flash schedule.
const DEEPSEEK_FLASH: PricingSchedule = PricingSchedule {
    cutoff_ms: DEEPSEEK_V4_1_FLASH_MS,
    peak_weekdays: 1..=5,
    peak_hours: &[1..4, 6..10],
    before: ScheduledRates {
        input: 0.3e-6,
        output: 1.2e-6,
        cache_create: 0.3e-6,
        cache_read: 0.006e-6,
        long_context: None,
    },
    off_peak: ScheduledRates {
        input: 0.15e-6,
        output: 0.6e-6,
        cache_create: 0.15e-6,
        cache_read: 0.003e-6,
        long_context: None,
    },
    peak: ScheduledRates {
        input: 0.3e-6,
        output: 1.2e-6,
        cache_create: 0.3e-6,
        cache_read: 0.006e-6,
        long_context: None,
    },
    successor: None,
    follows_row: false,
    vendor: "deepseek",
};

// NOTE(2026-11-21, #2227): xAI listed grok-code-fast-1 as its own model at $0.20 input, $0.02 cached and $1.50 output per 1M with no long-context rate in its console price data, last at https://web.archive.org/web/20260515145912/https://docs.x.ai/developers/pricing, and as an alias of grok-build-0.1 at $1.00 / $0.20 / $2.00, and $2.00 / $0.40 / $4.00 at 200K and above, from https://web.archive.org/web/20260521081415/https://docs.x.ai/developers/pricing on. xAI publishes no switch date, so the new rates start at the last capture that still shows the old ones (the earliest instant they can apply), which bills the few days in between at the higher rate. xAI bills no cache writes, so a write costs the input rate.
const GROK_CODE_FAST_1_AS_GROK_BUILD: ScheduledRates = ScheduledRates {
    input: 1e-6,
    output: 2e-6,
    cache_create: 1e-6,
    cache_read: 0.2e-6,
    long_context: Some(LongContext {
        threshold: XAI_LONG_CONTEXT_THRESHOLD,
        mode: LongContextMode::WholeRequest,
        rates: LongContextRates {
            input: Some(2e-6),
            output: Some(4e-6),
            cache_create: Some(2e-6),
            cache_create_1h: None,
            cache_read: Some(0.4e-6),
        },
    }),
};
const GROK_CODE_FAST_1: PricingSchedule = PricingSchedule {
    cutoff_ms: 1_778_857_152_000,
    peak_weekdays: 1..=5,
    peak_hours: &[],
    before: ScheduledRates {
        input: 0.2e-6,
        output: 1.5e-6,
        cache_create: 0.2e-6,
        cache_read: 0.02e-6,
        long_context: None,
    },
    off_peak: GROK_CODE_FAST_1_AS_GROK_BUILD,
    peak: GROK_CODE_FAST_1_AS_GROK_BUILD,
    successor: None,
    follows_row: false,
    vendor: "xai",
};

// NOTE(2026-11-21, #2455): A schedule is a vendor's own price list, so on a recorded billing route it applies only to usage that vendor billed itself. These provider IDs name a vendor's own API: DeepSeek's and xAI's LiteLLM provider IDs (https://docs.litellm.ai/docs/providers/deepseek, https://docs.litellm.ai/docs/providers/xai) and DeepSeek Harness's `deepseek-official` and `deepseek-account` routes. Any other provider ID bills at its own flat rate.
pub(super) static DIRECT_BILLING_PROVIDERS: &[(&str, &str)] = &[
    ("deepseek", "deepseek"),
    ("deepseek-official", "deepseek"),
    ("deepseek-account", "deepseek"),
    ("xai", "xai"),
];

pub(super) static SCHEDULES: &[(&str, PricingSchedule)] = &[
    ("deepseek-flash", DEEPSEEK_FLASH),
    ("deepseek-v4-flash", DEEPSEEK_V4_FLASH),
    // NOTE(2026-11-20, #2401): DeepSeek's August 21, 2026 V4 Flash Vision Exp release prices only image tokens at V4-Flash rates: https://api-docs.deepseek.com/news/news260821/. The alias therefore shares the V4 Flash schedule, including the September 10, 2026 handoff.
    ("deepseek-v4-flash-vision-exp", DEEPSEEK_V4_FLASH),
    ("deepseek-v4-pro", DEEPSEEK_V4_PRO),
    ("grok-code-fast-1", GROK_CODE_FAST_1),
    ("grok-code-fast", GROK_CODE_FAST_1),
    ("grok-code-fast-1-0825", GROK_CODE_FAST_1),
];
