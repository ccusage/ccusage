use ccusage_cli::{
    CodexSpeed, CostMode, CostSource, PricingOverride, SortOrder, VisualBurnRate, WeekDay,
};

use crate::config_schema::{
    ConfigCodexSpeed, ConfigCostMode, ConfigCostSource, ConfigPricingOverride, ConfigSortOrder,
    ConfigVisualBurnRate, ConfigWeekDay,
};

impl From<ConfigPricingOverride> for PricingOverride {
    fn from(value: ConfigPricingOverride) -> Self {
        Self {
            input_cost_per_token: value.input_cost_per_token,
            output_cost_per_token: value.output_cost_per_token,
            cache_creation_input_token_cost: value.cache_creation_input_token_cost,
            cache_creation_input_token_cost_above_1hr: value
                .cache_creation_input_token_cost_above_1hr,
            cache_read_input_token_cost: value.cache_read_input_token_cost,
            input_cost_per_token_above_200k_tokens: value.input_cost_per_token_above_200k_tokens,
            output_cost_per_token_above_200k_tokens: value.output_cost_per_token_above_200k_tokens,
            cache_creation_input_token_cost_above_200k_tokens: value
                .cache_creation_input_token_cost_above_200k_tokens,
            cache_read_input_token_cost_above_200k_tokens: value
                .cache_read_input_token_cost_above_200k_tokens,
            max_input_tokens: value.max_input_tokens,
            fast_multiplier: value.fast_multiplier,
            flex_multiplier: value.flex_multiplier,
        }
    }
}

impl From<ConfigCostMode> for CostMode {
    fn from(value: ConfigCostMode) -> Self {
        match value {
            ConfigCostMode::Auto => Self::Auto,
            ConfigCostMode::Calculate => Self::Calculate,
            ConfigCostMode::Display => Self::Display,
        }
    }
}

impl From<ConfigSortOrder> for SortOrder {
    fn from(value: ConfigSortOrder) -> Self {
        match value {
            ConfigSortOrder::Desc => Self::Desc,
            ConfigSortOrder::Asc => Self::Asc,
        }
    }
}

impl From<ConfigWeekDay> for WeekDay {
    fn from(value: ConfigWeekDay) -> Self {
        match value {
            ConfigWeekDay::Sunday => Self::Sunday,
            ConfigWeekDay::Monday => Self::Monday,
            ConfigWeekDay::Tuesday => Self::Tuesday,
            ConfigWeekDay::Wednesday => Self::Wednesday,
            ConfigWeekDay::Thursday => Self::Thursday,
            ConfigWeekDay::Friday => Self::Friday,
            ConfigWeekDay::Saturday => Self::Saturday,
        }
    }
}

impl From<ConfigCodexSpeed> for CodexSpeed {
    fn from(value: ConfigCodexSpeed) -> Self {
        match value {
            ConfigCodexSpeed::Auto => Self::Auto,
            ConfigCodexSpeed::Standard => Self::Standard,
            ConfigCodexSpeed::Fast => Self::Fast,
            ConfigCodexSpeed::Flex => Self::Flex,
        }
    }
}

impl From<ConfigVisualBurnRate> for VisualBurnRate {
    fn from(value: ConfigVisualBurnRate) -> Self {
        match value {
            ConfigVisualBurnRate::Off => Self::Off,
            ConfigVisualBurnRate::Emoji => Self::Emoji,
            ConfigVisualBurnRate::Text => Self::Text,
            ConfigVisualBurnRate::EmojiText => Self::EmojiText,
        }
    }
}

impl From<ConfigCostSource> for CostSource {
    fn from(value: ConfigCostSource) -> Self {
        match value {
            ConfigCostSource::Auto => Self::Auto,
            ConfigCostSource::Ccusage => Self::Ccusage,
            ConfigCostSource::Cc => Self::Cc,
            ConfigCostSource::Both => Self::Both,
        }
    }
}
