use std::collections::{BTreeMap, HashMap};

use ccusage_cli::{SessionLength, TokenLimit};
use serde::{Deserialize, Deserializer};

#[cfg(any(feature = "schema", test))]
pub(crate) mod generator;

pub(crate) const NAMED_PI_STORE_NAME_PATTERN: &str = "^[a-z][a-z0-9_-]{0,31}$";

#[derive(Debug, Default, Deserialize)]
#[cfg_attr(any(feature = "schema", test), derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub(crate) struct PiStoreConfig {
    /// Agent name to use for this pi-format store in all-agent reports.
    #[cfg_attr(
        any(feature = "schema", test),
        schemars(regex(pattern = "^[a-z][a-z0-9_-]{0,31}$"))
    )]
    pub(crate) name: String,
    /// Path to this pi-format sessions directory. Comma-separated lists are supported.
    pub(crate) path: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ConfigDateBound(pub(crate) ccusage_cli::DateBound);

impl<'de> Deserialize<'de> for ConfigDateBound {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        ccusage_cli::DateBound::parse(&value)
            .map(Self)
            .ok_or_else(|| {
                serde::de::Error::custom(format!(
                    "not a valid date. Expected {}",
                    ccusage_cli::DATE_BOUND_FORMATS
                ))
            })
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[cfg_attr(any(feature = "schema", test), derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub(crate) struct SharedOptions {
    /// Filter from date (YYYY-MM-DD or YYYYMMDD).
    #[cfg_attr(any(feature = "schema", test), schemars(with = "Option<String>"))]
    pub(crate) since: Option<ConfigDateBound>,
    /// Filter until date (inclusive).
    #[cfg_attr(any(feature = "schema", test), schemars(with = "Option<String>"))]
    pub(crate) until: Option<ConfigDateBound>,
    /// Output in JSON format.
    pub(crate) json: Option<bool>,
    /// Cost calculation mode.
    pub(crate) mode: Option<ConfigCostMode>,
    /// Show pricing mismatch information for debugging.
    pub(crate) debug: Option<bool>,
    /// Number of sample discrepancies to show in debug output.
    pub(crate) debug_samples: Option<usize>,
    /// Sort order.
    pub(crate) order: Option<ConfigSortOrder>,
    /// Show per-model cost breakdown.
    pub(crate) breakdown: Option<bool>,
    /// Use cached pricing data where supported.
    pub(crate) offline: Option<bool>,
    /// Disable cached pricing data where supported.
    pub(crate) no_offline: Option<bool>,
    /// Enable colored output.
    pub(crate) color: Option<bool>,
    /// Disable colored output.
    pub(crate) no_color: Option<bool>,
    /// Timezone for date grouping (IANA).
    pub(crate) timezone: Option<String>,
    /// jq filter to apply to JSON output.
    pub(crate) jq: Option<String>,
    /// Force compact table layout for narrow terminals.
    pub(crate) compact: Option<bool>,
    /// Disable parallel file processing.
    pub(crate) single_thread: Option<bool>,
    /// Hide cost information in table and JSON output.
    pub(crate) no_cost: Option<bool>,
    /// Runtime pricing overrides keyed by raw model name.
    pub(crate) pricing_overrides: Option<BTreeMap<String, ConfigPricingOverride>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[cfg_attr(any(feature = "schema", test), derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub(crate) struct DailySpecificOptions {
    /// Show per-session project instances.
    pub(crate) instances: Option<bool>,
    /// JSON object or path mapping project paths to display aliases.
    pub(crate) project_aliases: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[cfg_attr(any(feature = "schema", test), derive(schemars::JsonSchema))]
pub(crate) struct ClaudeOptions {
    /// Filter Claude Code daily, weekly, monthly, session, and blocks reports to one project.
    pub(crate) project: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[cfg_attr(any(feature = "schema", test), derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub(crate) struct WeeklySpecificOptions {
    /// First day of week for weekly grouping.
    pub(crate) start_of_week: Option<ConfigWeekDay>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[cfg_attr(any(feature = "schema", test), derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub(crate) struct BlocksSpecificOptions {
    /// Show the active session block.
    pub(crate) active: Option<bool>,
    /// Show recent session blocks.
    pub(crate) recent: Option<bool>,
    /// Token limit for session block calculations.
    #[serde(default, deserialize_with = "deserialize_token_limit")]
    #[cfg_attr(
        any(feature = "schema", test),
        schemars(with = "Option<ConfigTokenLimit>")
    )]
    pub(crate) token_limit: Option<TokenLimit>,
    /// Session block duration in hours.
    #[serde(default, deserialize_with = "deserialize_session_length")]
    #[cfg_attr(any(feature = "schema", test), schemars(with = "Option<f64>"))]
    pub(crate) session_length: Option<SessionLength>,
}

#[derive(Clone, Debug, Deserialize)]
#[cfg_attr(any(feature = "schema", test), derive(schemars::JsonSchema))]
#[serde(untagged)]
pub(crate) enum ConfigTokenLimit {
    Text(
        #[cfg_attr(
            any(feature = "schema", test),
            schemars(regex(pattern = r"^(max|\+?0*[1-9][0-9]*)$"))
        )]
        String,
    ),
    Number(#[cfg_attr(any(feature = "schema", test), schemars(range(min = 1)))] u64),
}

fn deserialize_token_limit<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<TokenLimit>, D::Error> {
    Option::<ConfigTokenLimit>::deserialize(deserializer)?
        .map(|value| {
            let text = match value {
                ConfigTokenLimit::Text(value) => value,
                ConfigTokenLimit::Number(value) => value.to_string(),
            };
            text.parse().map_err(serde::de::Error::custom)
        })
        .transpose()
}

fn deserialize_session_length<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<SessionLength>, D::Error> {
    Option::<f64>::deserialize(deserializer)?
        .map(|value| SessionLength::from_hours(value).map_err(serde::de::Error::custom))
        .transpose()
}

#[derive(Clone, Debug, Default, Deserialize)]
#[cfg_attr(any(feature = "schema", test), derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatuslineSpecificOptions {
    /// Use cached pricing data where supported.
    pub(crate) offline: Option<bool>,
    /// Disable cached pricing data where supported.
    pub(crate) no_offline: Option<bool>,
    /// Visual burn-rate display mode.
    pub(crate) visual_burn_rate: Option<ConfigVisualBurnRate>,
    /// Source for statusline cost calculation.
    pub(crate) cost_source: Option<ConfigCostSource>,
    /// Enable statusline cache.
    pub(crate) cache: Option<bool>,
    /// Disable statusline cache.
    pub(crate) no_cache: Option<bool>,
    /// Statusline refresh interval in seconds.
    pub(crate) refresh_interval: Option<u64>,
    /// Percentage threshold for low context warning.
    #[cfg_attr(any(feature = "schema", test), schemars(with = "Option<u64>"))]
    pub(crate) context_low_threshold: Option<u8>,
    /// Percentage threshold for medium context warning.
    #[cfg_attr(any(feature = "schema", test), schemars(with = "Option<u64>"))]
    pub(crate) context_medium_threshold: Option<u8>,
    /// Timezone for date grouping (IANA).
    pub(crate) timezone: Option<String>,
    /// Show statusline debug information.
    pub(crate) debug: Option<bool>,
    /// Map model identifiers to short display labels.
    pub(crate) model_label_aliases: Option<HashMap<String, String>>,
    /// Runtime pricing overrides keyed by raw model name.
    pub(crate) pricing_overrides: Option<BTreeMap<String, ConfigPricingOverride>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[cfg_attr(any(feature = "schema", test), derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub(crate) struct CodexOptions {
    /// Codex speed normalization strategy.
    pub(crate) speed: Option<ConfigCodexSpeed>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[cfg_attr(any(feature = "schema", test), derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub(crate) struct PiOptions {
    /// Path or comma-separated paths to pi-agent sessions directories.
    pub(crate) pi_path: Option<String>,
    /// Path or comma-separated paths to pi agent directories; ccusage reads each directory's `sessions` store.
    pub(crate) pi_agent_dir: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[cfg_attr(any(feature = "schema", test), derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenClawOptions {
    /// Path or comma-separated paths to OpenClaw data directories.
    pub(crate) open_claw_path: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[cfg_attr(any(feature = "schema", test), derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum ConfigCostMode {
    Auto,
    Calculate,
    Display,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[cfg_attr(any(feature = "schema", test), derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum ConfigSortOrder {
    Desc,
    Asc,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[cfg_attr(any(feature = "schema", test), derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum ConfigWeekDay {
    Sunday,
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[cfg_attr(any(feature = "schema", test), derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum ConfigCodexSpeed {
    Auto,
    Standard,
    Fast,
    Flex,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[cfg_attr(any(feature = "schema", test), derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum ConfigVisualBurnRate {
    Off,
    Emoji,
    Text,
    EmojiText,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[cfg_attr(any(feature = "schema", test), derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum ConfigCostSource {
    Auto,
    Ccusage,
    Cc,
    Both,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[cfg_attr(any(feature = "schema", test), derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ConfigPricingOverride {
    pub(crate) input_cost_per_token: Option<f64>,
    pub(crate) output_cost_per_token: Option<f64>,
    pub(crate) cache_creation_input_token_cost: Option<f64>,
    pub(crate) cache_creation_input_token_cost_above_1hr: Option<f64>,
    pub(crate) cache_read_input_token_cost: Option<f64>,
    pub(crate) input_cost_per_token_above_200k_tokens: Option<f64>,
    pub(crate) output_cost_per_token_above_200k_tokens: Option<f64>,
    pub(crate) cache_creation_input_token_cost_above_200k_tokens: Option<f64>,
    pub(crate) cache_read_input_token_cost_above_200k_tokens: Option<f64>,
    pub(crate) max_input_tokens: Option<u64>,
    pub(crate) fast_multiplier: Option<f64>,
    pub(crate) flex_multiplier: Option<f64>,
}

#[cfg(test)]
mod tests;
