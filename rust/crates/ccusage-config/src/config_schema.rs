use std::collections::{BTreeMap, HashMap};

use ccusage_cli::{SessionLength, TokenLimit};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value, json};

use schemars::{JsonSchema, r#gen::SchemaSettings};

pub(crate) const NAMED_PI_STORE_NAME_PATTERN: &str = "^[a-z][a-z0-9_-]{0,31}$";

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CcusageConfig {
    /// JSON Schema URL for validation and autocomplete.
    #[serde(rename = "$schema")]
    schema_url: Option<String>,
    /// Default values for all-agent reports and legacy Claude commands.
    defaults: Option<SharedOptions>,
    /// Command-specific configuration for all-agent reports.
    commands: Option<RootCommandsConfig>,
    /// Claude Code configuration.
    claude: Option<ClaudeConfig>,
    /// Cline configuration.
    cline: Option<ClineConfig>,
    /// Codex configuration.
    codex: Option<CodexConfig>,
    /// OpenCode configuration.
    opencode: Option<OpenCodeConfig>,
    /// Amp configuration.
    amp: Option<AmpConfig>,
    /// Droid configuration.
    droid: Option<DroidConfig>,
    /// Codebuff configuration.
    codebuff: Option<CodebuffConfig>,
    /// Hermes Agent configuration.
    hermes: Option<HermesConfig>,
    /// pi-agent configuration.
    pi: Option<PiConfig>,
    /// Goose configuration.
    goose: Option<GooseConfig>,
    /// OpenClaw configuration.
    openclaw: Option<OpenClawConfig>,
    /// Kilo configuration.
    kilo: Option<KiloConfig>,
    /// GitHub Copilot CLI configuration.
    copilot: Option<CopilotConfig>,
    /// Gemini CLI configuration.
    gemini: Option<GeminiConfig>,
    /// Antigravity configuration.
    antigravity: Option<AntigravityConfig>,
    /// Kimi configuration.
    kimi: Option<KimiConfig>,
    /// Qwen configuration.
    qwen: Option<QwenConfig>,
    /// Grok Build CLI configuration.
    grok: Option<GrokConfig>,
    /// ZCode configuration.
    zcode: Option<ZCodeConfig>,
    /// DeepSeek Harness configuration.
    dsh: Option<DshConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootCommandsConfig {
    daily: Option<DailyOptions>,
    weekly: Option<WeeklyOptions>,
    monthly: Option<SharedOptions>,
    session: Option<SharedOptions>,
    blocks: Option<BlocksOptions>,
    statusline: Option<StatuslineOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeConfig {
    defaults: Option<ClaudeOptions>,
    commands: Option<ClaudeCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeCommandsConfig {
    daily: Option<DailyOptions>,
    weekly: Option<ClaudeWeeklyOptions>,
    monthly: Option<SharedOptions>,
    session: Option<SharedOptions>,
    blocks: Option<BlocksOptions>,
    statusline: Option<StatuslineOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexConfig {
    defaults: Option<CodexOptions>,
    commands: Option<CodexCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexCommandsConfig {
    daily: Option<CodexOptions>,
    weekly: Option<CodexOptions>,
    monthly: Option<CodexOptions>,
    session: Option<CodexOptions>,
    statusline: Option<CodexStatuslineOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCodeConfig {
    defaults: Option<SharedOptions>,
    commands: Option<OpenCodeCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCodeCommandsConfig {
    daily: Option<SharedOptions>,
    weekly: Option<SharedOptions>,
    monthly: Option<SharedOptions>,
    session: Option<SharedOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AmpConfig {
    defaults: Option<SharedOptions>,
    commands: Option<AmpCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AmpCommandsConfig {
    daily: Option<SharedOptions>,
    weekly: Option<SharedOptions>,
    monthly: Option<SharedOptions>,
    session: Option<SharedOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DroidConfig {
    defaults: Option<SharedOptions>,
    commands: Option<DroidCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DroidCommandsConfig {
    daily: Option<SharedOptions>,
    weekly: Option<SharedOptions>,
    monthly: Option<SharedOptions>,
    session: Option<SharedOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodebuffConfig {
    defaults: Option<SharedOptions>,
    commands: Option<CodebuffCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodebuffCommandsConfig {
    daily: Option<SharedOptions>,
    weekly: Option<SharedOptions>,
    monthly: Option<SharedOptions>,
    session: Option<SharedOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClineConfig {
    defaults: Option<SharedOptions>,
    commands: Option<ClineCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClineCommandsConfig {
    daily: Option<SharedOptions>,
    weekly: Option<SharedOptions>,
    monthly: Option<SharedOptions>,
    session: Option<SharedOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DshConfig {
    defaults: Option<SharedOptions>,
    commands: Option<DshCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DshCommandsConfig {
    daily: Option<SharedOptions>,
    weekly: Option<SharedOptions>,
    monthly: Option<SharedOptions>,
    session: Option<SharedOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesConfig {
    defaults: Option<SharedOptions>,
    commands: Option<HermesCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesCommandsConfig {
    daily: Option<SharedOptions>,
    weekly: Option<SharedOptions>,
    monthly: Option<SharedOptions>,
    session: Option<SharedOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PiConfig {
    /// Additional named pi-format session stores included in all-agent reports.
    stores: Option<Vec<PiStoreConfig>>,
    defaults: Option<PiOptions>,
    commands: Option<PiCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PiStoreConfig {
    /// Agent name to use for this pi-format store in all-agent reports.
    #[schemars(regex(pattern = "^[a-z][a-z0-9_-]{0,31}$"))]
    pub(crate) name: String,
    /// Path to this pi-format sessions directory. Comma-separated lists are supported.
    pub(crate) path: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PiCommandsConfig {
    daily: Option<PiOptions>,
    weekly: Option<PiOptions>,
    monthly: Option<PiOptions>,
    session: Option<PiOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GooseConfig {
    defaults: Option<SharedOptions>,
    commands: Option<GooseCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GooseCommandsConfig {
    daily: Option<SharedOptions>,
    weekly: Option<SharedOptions>,
    monthly: Option<SharedOptions>,
    session: Option<SharedOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenClawConfig {
    defaults: Option<OpenClawOptions>,
    commands: Option<OpenClawCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenClawCommandsConfig {
    daily: Option<OpenClawOptions>,
    weekly: Option<OpenClawOptions>,
    monthly: Option<OpenClawOptions>,
    session: Option<OpenClawOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KiloConfig {
    defaults: Option<SharedOptions>,
    commands: Option<KiloCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KiloCommandsConfig {
    daily: Option<SharedOptions>,
    weekly: Option<SharedOptions>,
    monthly: Option<SharedOptions>,
    session: Option<SharedOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CopilotConfig {
    defaults: Option<SharedOptions>,
    commands: Option<CopilotCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CopilotCommandsConfig {
    daily: Option<SharedOptions>,
    weekly: Option<SharedOptions>,
    monthly: Option<SharedOptions>,
    session: Option<SharedOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiConfig {
    defaults: Option<SharedOptions>,
    commands: Option<GeminiCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiCommandsConfig {
    daily: Option<SharedOptions>,
    weekly: Option<SharedOptions>,
    monthly: Option<SharedOptions>,
    session: Option<SharedOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AntigravityConfig {
    defaults: Option<SharedOptions>,
    commands: Option<AntigravityCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AntigravityCommandsConfig {
    daily: Option<SharedOptions>,
    weekly: Option<SharedOptions>,
    monthly: Option<SharedOptions>,
    session: Option<SharedOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KimiConfig {
    defaults: Option<SharedOptions>,
    commands: Option<KimiCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KimiCommandsConfig {
    daily: Option<SharedOptions>,
    weekly: Option<SharedOptions>,
    monthly: Option<SharedOptions>,
    session: Option<SharedOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QwenConfig {
    defaults: Option<SharedOptions>,
    commands: Option<QwenCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QwenCommandsConfig {
    daily: Option<SharedOptions>,
    weekly: Option<SharedOptions>,
    monthly: Option<SharedOptions>,
    session: Option<SharedOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrokConfig {
    defaults: Option<SharedOptions>,
    commands: Option<GrokCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrokCommandsConfig {
    daily: Option<SharedOptions>,
    weekly: Option<SharedOptions>,
    monthly: Option<SharedOptions>,
    session: Option<SharedOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZCodeConfig {
    defaults: Option<SharedOptions>,
    commands: Option<ZCodeCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZCodeCommandsConfig {
    daily: Option<SharedOptions>,
    weekly: Option<SharedOptions>,
    monthly: Option<SharedOptions>,
    session: Option<SharedOptions>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConfigDateBound(pub(crate) String);

impl<'de> Deserialize<'de> for ConfigDateBound {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        ccusage_cli::normalize_date_bound(&value)
            .map(Self)
            .ok_or_else(|| {
                serde::de::Error::custom(format!(
                    "not a valid date. Expected {}",
                    ccusage_cli::DATE_BOUND_FORMATS
                ))
            })
    }
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedOptions {
    /// Filter from date (YYYY-MM-DD or YYYYMMDD).
    #[schemars(with = "Option<String>")]
    pub(crate) since: Option<ConfigDateBound>,
    /// Filter until date (inclusive).
    #[schemars(with = "Option<String>")]
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
    /// Accepted for compatibility; all detected supported agents are included by default.
    all: Option<bool>,
    /// Force compact table layout for narrow terminals.
    pub(crate) compact: Option<bool>,
    /// Disable parallel file processing.
    pub(crate) single_thread: Option<bool>,
    /// Hide cost information in table and JSON output.
    pub(crate) no_cost: Option<bool>,
    /// Runtime pricing overrides keyed by raw model name.
    pub(crate) pricing_overrides: Option<BTreeMap<String, ConfigPricingOverride>>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeOptions {
    #[serde(flatten)]
    shared: SharedOptions,
    #[serde(flatten)]
    daily: DailySpecificOptions,
    #[serde(flatten)]
    weekly: WeeklySpecificOptions,
    #[serde(flatten)]
    blocks: BlocksSpecificOptions,
    #[serde(flatten)]
    statusline: StatuslineSpecificOptions,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyOptions {
    #[serde(flatten)]
    shared: SharedOptions,
    #[serde(flatten)]
    daily: DailySpecificOptions,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DailySpecificOptions {
    /// Show per-session project instances.
    pub(crate) instances: Option<bool>,
    /// Filter to a project name or path.
    pub(crate) project: Option<String>,
    /// JSON object or path mapping project paths to display aliases.
    pub(crate) project_aliases: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeeklyOptions {
    #[serde(flatten)]
    shared: SharedOptions,
    #[serde(flatten)]
    weekly: WeeklySpecificOptions,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeWeeklyOptions {
    #[serde(flatten)]
    shared: SharedOptions,
    #[serde(flatten)]
    weekly: WeeklySpecificOptions,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeeklySpecificOptions {
    /// First day of week for weekly grouping.
    pub(crate) start_of_week: Option<ConfigWeekDay>,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlocksOptions {
    #[serde(flatten)]
    shared: SharedOptions,
    #[serde(flatten)]
    blocks: BlocksSpecificOptions,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlocksSpecificOptions {
    /// Show the active session block.
    pub(crate) active: Option<bool>,
    /// Show recent session blocks.
    pub(crate) recent: Option<bool>,
    /// Token limit for session block calculations.
    #[serde(
        default,
        deserialize_with = "deserialize_token_limit",
        serialize_with = "serialize_token_limit",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<ConfigTokenLimit>")]
    pub(crate) token_limit: Option<TokenLimit>,
    /// Session block duration in hours.
    #[serde(
        default,
        deserialize_with = "deserialize_session_length",
        serialize_with = "serialize_session_length",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<f64>")]
    pub(crate) session_length: Option<SessionLength>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(untagged)]
pub enum ConfigTokenLimit {
    Text(#[schemars(regex(pattern = r"^(max|\+?0*[1-9][0-9]*)$"))] String),
    Number(#[schemars(range(min = 1))] u64),
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

fn serialize_token_limit<S: Serializer>(
    limit: &Option<TokenLimit>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    limit.map(|limit| limit.to_string()).serialize(serializer)
}

fn serialize_session_length<S: Serializer>(
    length: &Option<SessionLength>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    length
        .map(|length| {
            std::time::Duration::from_millis(length.as_millis().unsigned_abs()).as_secs_f64()
                / 3600.0
        })
        .serialize(serializer)
}

fn serialize_example(example: CcusageConfig) -> serde_json::Result<Value> {
    fn remove_unset(value: &mut Value) {
        match value {
            Value::Object(fields) => {
                fields.retain(|_, value| !value.is_null());
                fields.values_mut().for_each(remove_unset);
            }
            Value::Array(items) => items.iter_mut().for_each(remove_unset),
            _ => {}
        }
    }
    let mut value = serde_json::to_value(example)?;
    remove_unset(&mut value);
    Ok(value)
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatuslineOptions {
    #[serde(flatten)]
    statusline: StatuslineSpecificOptions,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatuslineSpecificOptions {
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
    #[schemars(with = "Option<u64>")]
    pub(crate) context_low_threshold: Option<u8>,
    /// Percentage threshold for medium context warning.
    #[schemars(with = "Option<u64>")]
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

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexOptions {
    #[serde(flatten)]
    shared: SharedOptions,
    /// Codex speed normalization strategy.
    pub(crate) speed: Option<ConfigCodexSpeed>,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexStatuslineOptions {
    json: Option<bool>,
    jq: Option<String>,
    mode: Option<ConfigCostMode>,
    no_cost: Option<bool>,
    #[schemars(with = "Option<String>")]
    since: Option<ConfigDateBound>,
    #[schemars(with = "Option<String>")]
    until: Option<ConfigDateBound>,
    timezone: Option<String>,
    offline: Option<bool>,
    no_offline: Option<bool>,
    single_thread: Option<bool>,
    speed: Option<ConfigCodexSpeed>,
    pricing_overrides: Option<BTreeMap<String, ConfigPricingOverride>>,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PiOptions {
    #[serde(flatten)]
    shared: SharedOptions,
    /// Path or comma-separated paths to pi-agent sessions directories.
    pub(crate) pi_path: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenClawOptions {
    #[serde(flatten)]
    shared: SharedOptions,
    /// Path or comma-separated paths to OpenClaw data directories.
    pub(crate) open_claw_path: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigCostMode {
    Auto,
    Calculate,
    Display,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigSortOrder {
    Desc,
    Asc,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, Serialize)]
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

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigCodexSpeed {
    Auto,
    Standard,
    Fast,
    Flex,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigVisualBurnRate {
    Off,
    Emoji,
    Text,
    EmojiText,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigCostSource {
    Auto,
    Ccusage,
    Cc,
    Both,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Serialize, Clone)]
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

pub fn generate_config_schema_json() -> serde_json::Result<String> {
    let generator = SchemaSettings::draft07()
        .with(|settings| {
            settings.meta_schema = Some("https://json-schema.org/draft-07/schema#".to_string());
            settings.option_add_null_type = false;
        })
        .into_generator();
    let mut schema = serde_json::to_value(generator.into_root_schema_for::<CcusageConfig>())?;
    if let Value::Object(root) = &mut schema {
        root.insert(
            "title".to_string(),
            Value::String("ccusage Configuration".to_string()),
        );
        root.insert(
            "description".to_string(),
            Value::String("Configuration file for ccusage".to_string()),
        );
        root.insert(
            "examples".to_string(),
            Value::Array(vec![
                serde_json::from_value(json!(
                    {
                        "$schema": "https://ccusage.com/config-schema.json",
                        "defaults": {
                            "json": false,
                            "timezone": "Asia/Tokyo",
                            "pricingOverrides": {
                                "[pi] gpt-5.4": {
                                    "inputCostPerToken": 0.0000025,
                                    "outputCostPerToken": 0.000015,
                                    "cacheReadInputTokenCost": 0.00000025
                                }
                            }
                        },
                        "claude": {
                            "defaults": {
                                "mode": "auto"
                            },
                            "commands": {
                                "daily": {
                                    "instances": true
                                },
                                "blocks": {
                                    "tokenLimit": "500000"
                                }
                            }
                        },
                        "codex": {
                            "defaults": {
                                "speed": "auto"
                            }
                        },
                        "pi": {
                            "stores": [
                                {
                                    "name": "omp",
                                    "path": "~/.omp/agent/sessions"
                                }
                            ]
                        },
                        "gemini": {
                            "defaults": {
                                "offline": true
                            }
                        }
                    }
                ))
                .and_then(serialize_example)?,
            ]),
        );
    }
    enrich_schema(&mut schema);
    add_schema_defaults(&mut schema);
    inline_schema_references(&mut schema);
    if let Some(offline) = schema.pointer_mut(
        "/properties/codex/properties/commands/properties/statusline/properties/offline",
    ) && let Some(command) = ccusage_cli::AgentId::Codex
        .spec()
        .extra_command("statusline")
    {
        offline["default"] = json!(command.offline_by_default());
    }
    wrap_root_schema(&mut schema);
    add_agent_command_keys(&mut schema);
    let mut json = tab_indent_json(&serde_json::to_string_pretty(&schema)?);
    json.push('\n');
    Ok(json)
}

fn tab_indent_json(json: &str) -> String {
    json.lines()
        .map(|line| {
            let spaces = line
                .as_bytes()
                .iter()
                .take_while(|byte| **byte == b' ')
                .count();
            let mut formatted = "\t".repeat(spaces / 2);
            formatted.push_str(&line[spaces..]);
            formatted
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn enrich_schema(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(description) = map.get("description").cloned() {
                map.entry("markdownDescription".to_string())
                    .or_insert(description);
            }
            if map.contains_key("properties") {
                map.entry("additionalProperties".to_string())
                    .or_insert(Value::Bool(false));
            }
            for child in map.values_mut() {
                enrich_schema(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                enrich_schema(child);
            }
        }
        _ => {}
    }
}

fn add_schema_defaults(schema: &mut Value) {
    set_definition_defaults(
        schema,
        "SharedOptions",
        &[
            ("json", json!(false)),
            ("mode", json!("auto")),
            ("debug", json!(false)),
            ("debugSamples", json!(5)),
            ("order", json!("asc")),
            ("breakdown", json!(false)),
            ("offline", json!(false)),
            ("noOffline", json!(false)),
            ("color", json!(false)),
            ("noColor", json!(false)),
            ("all", json!(false)),
            ("compact", json!(false)),
            ("singleThread", json!(false)),
            ("noCost", json!(false)),
        ],
    );
    set_definition_defaults(schema, "WeeklyOptions", &[("startOfWeek", json!("monday"))]);
    set_definition_defaults(
        schema,
        "ClaudeWeeklyOptions",
        &[("startOfWeek", json!("sunday"))],
    );
    set_definition_defaults(schema, "BlocksOptions", &[("sessionLength", json!(5.0))]);
    set_definition_defaults(
        schema,
        "StatuslineOptions",
        &[
            ("offline", json!(true)),
            ("noOffline", json!(false)),
            ("visualBurnRate", json!("off")),
            ("costSource", json!("auto")),
            ("cache", json!(true)),
            ("noCache", json!(false)),
            ("refreshInterval", json!(1)),
            ("contextLowThreshold", json!(50)),
            ("contextMediumThreshold", json!(80)),
            ("debug", json!(false)),
        ],
    );
    set_definition_defaults(schema, "CodexOptions", &[("speed", json!("auto"))]);
}

fn set_definition_defaults(schema: &mut Value, definition: &str, defaults: &[(&str, Value)]) {
    let Some(properties) = schema
        .get_mut("definitions")
        .and_then(|definitions| definitions.get_mut(definition))
        .and_then(|definition| definition.get_mut("properties"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    for (property, default) in defaults {
        if let Some(property_schema) = properties.get_mut(*property).and_then(Value::as_object_mut)
        {
            property_schema
                .entry("default".to_string())
                .or_insert_with(|| default.clone());
        }
    }
}

fn inline_schema_references(schema: &mut Value) {
    let definitions = schema
        .get("definitions")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    inline_schema_value(schema, &definitions);
}

fn inline_schema_value(value: &mut Value, definitions: &Map<String, Value>) {
    match value {
        Value::Object(map) => {
            inline_ref(map, definitions);
            inline_all_of(map, definitions);
            for child in map.values_mut() {
                inline_schema_value(child, definitions);
            }
        }
        Value::Array(values) => {
            for child in values {
                inline_schema_value(child, definitions);
            }
        }
        _ => {}
    }
}

fn inline_ref(map: &mut Map<String, Value>, definitions: &Map<String, Value>) {
    let Some(reference) = map.remove("$ref") else {
        return;
    };
    let Some(reference) = reference.as_str() else {
        return;
    };
    let Some(definition_name) = reference.strip_prefix("#/definitions/") else {
        return;
    };
    let Some(Value::Object(definition)) = definitions.get(definition_name).cloned() else {
        return;
    };

    let existing = std::mem::take(map);
    for (key, value) in definition {
        map.insert(key, value);
    }
    for (key, value) in existing {
        map.insert(key, value);
    }
}

fn inline_all_of(map: &mut Map<String, Value>, definitions: &Map<String, Value>) {
    let Some(Value::Array(items)) = map.remove("allOf") else {
        return;
    };
    for mut item in items {
        inline_schema_value(&mut item, definitions);
        let Value::Object(item) = item else {
            continue;
        };
        merge_schema_object(map, item);
    }
}

fn merge_schema_object(target: &mut Map<String, Value>, source: Map<String, Value>) {
    for (key, value) in source {
        if key == "properties" {
            let target_properties = target
                .entry(key)
                .or_insert_with(|| Value::Object(Map::new()));
            if let (Some(target), Value::Object(source)) =
                (target_properties.as_object_mut(), value)
            {
                target.extend(source);
            }
            continue;
        }
        target.entry(key).or_insert(value);
    }
}

fn wrap_root_schema(schema: &mut Value) {
    let Value::Object(root) = schema else {
        return;
    };
    root.remove("definitions");
    let mut definitions = Map::new();
    let mut root_definition = Map::new();
    for key in [
        "additionalProperties",
        "description",
        "markdownDescription",
        "properties",
        "type",
    ] {
        if let Some(value) = root.remove(key) {
            root_definition.insert(key.to_string(), value);
        }
    }
    definitions.insert("ccusage-config".to_string(), Value::Object(root_definition));
    root.insert(
        "$ref".to_string(),
        Value::String("#/definitions/ccusage-config".to_string()),
    );
    root.insert("definitions".to_string(), Value::Object(definitions));
}

/// `commands."<agent> <report>"` and `commands."<agent>:<report>"` take the
/// options of `<agent>.commands.<report>`. They point at that schema instead of
/// inlining it, which would add a copy of every agent report schema per form.
fn add_agent_command_keys(schema: &mut Value) {
    let Some(config) = schema.pointer("/definitions/ccusage-config/properties") else {
        return;
    };
    let keys = config
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(agent, agent_schema)| {
            agent_schema
                .pointer("/properties/commands/properties")
                .and_then(Value::as_object)
                .map(|reports| (agent, reports))
        })
        .flat_map(|(agent, reports)| {
            reports.keys().flat_map(move |report| {
                let reference = json!({
                    "$ref": format!(
                        "#/definitions/ccusage-config/properties/{agent}/properties/commands/properties/{report}"
                    )
                });
                [
                    (format!("{agent} {report}"), reference.clone()),
                    (format!("{agent}:{report}"), reference),
                ]
            })
        })
        .collect::<Vec<_>>();
    if let Some(commands) = schema
        .pointer_mut("/definitions/ccusage-config/properties/commands/properties")
        .and_then(Value::as_object_mut)
    {
        commands.extend(keys);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::{Value, json};

    use super::StatuslineSpecificOptions;
    use super::generate_config_schema_json;

    #[test]
    fn schema_option_sets_expose_expected_keys() {
        let schema = generated_schema();
        let shared = [
            "all",
            "breakdown",
            "color",
            "compact",
            "debug",
            "debugSamples",
            "jq",
            "json",
            "mode",
            "noColor",
            "noCost",
            "noOffline",
            "offline",
            "order",
            "pricingOverrides",
            "since",
            "singleThread",
            "timezone",
            "until",
        ];

        assert_schema_properties(&schema, &["defaults"], &shared);
        assert_schema_properties(
            &schema,
            &["commands", "daily"],
            &with_keys(&shared, &["instances", "project", "projectAliases"]),
        );
        assert_schema_properties(
            &schema,
            &["commands", "weekly"],
            &with_keys(&shared, &["startOfWeek"]),
        );
        assert_schema_properties(
            &schema,
            &["commands", "blocks"],
            &with_keys(
                &shared,
                &["active", "recent", "sessionLength", "tokenLimit"],
            ),
        );
        assert_schema_properties(
            &schema,
            &["commands", "statusline"],
            &[
                "cache",
                "contextLowThreshold",
                "contextMediumThreshold",
                "costSource",
                "debug",
                "modelLabelAliases",
                "noCache",
                "noOffline",
                "offline",
                "pricingOverrides",
                "refreshInterval",
                "timezone",
                "visualBurnRate",
            ],
        );
        assert_schema_properties(
            &schema,
            &["codex", "defaults"],
            &with_keys(&shared, &["speed"]),
        );
        assert_schema_properties(
            &schema,
            &["pi", "defaults"],
            &with_keys(&shared, &["piPath"]),
        );
        assert!(schema_property(&schema, &["pi", "stores"]).is_some());
        assert_schema_properties(
            &schema,
            &["openclaw", "defaults"],
            &with_keys(&shared, &["openClawPath"]),
        );
        assert_schema_properties(&schema, &["grok", "defaults"], &shared);
        assert_schema_properties(&schema, &["antigravity", "defaults"], &shared);
        assert_schema_properties(&schema, &["zcode", "defaults"], &shared);
        assert_schema_properties(&schema, &["cline", "defaults"], &shared);
        assert_schema_properties(&schema, &["cline", "commands", "weekly"], &shared);
    }

    #[test]
    fn agent_configs_expose_only_supported_option_sets() {
        let schema = generated_schema();

        assert!(schema_property(&schema, &["codex", "defaults", "speed"]).is_some());
        assert!(schema_property(&schema, &["opencode", "defaults", "speed"]).is_none());
        assert!(schema_property(&schema, &["amp", "defaults", "speed"]).is_none());
        assert!(schema_property(&schema, &["droid", "defaults", "speed"]).is_none());
        assert!(schema_property(&schema, &["codebuff", "defaults", "speed"]).is_none());
        assert!(schema_property(&schema, &["pi", "defaults", "piPath"]).is_some());
        assert!(schema_property(&schema, &["pi", "stores"]).is_some());
        assert!(schema_property(&schema, &["goose", "defaults", "piPath"]).is_none());
        assert!(schema_property(&schema, &["openclaw", "defaults", "openClawPath"]).is_some());
        assert!(schema_property(&schema, &["kilo", "defaults", "openClawPath"]).is_none());
        assert!(schema_property(&schema, &["gemini", "defaults", "openClawPath"]).is_none());
        assert!(schema_property(&schema, &["antigravity", "defaults", "openClawPath"]).is_none());
        assert!(schema_property(&schema, &["kimi", "defaults", "openClawPath"]).is_none());
        assert!(schema_property(&schema, &["cline", "commands", "weekly"]).is_some());
        assert!(schema_property(&schema, &["qwen", "defaults", "openClawPath"]).is_none());
        assert!(schema_property(&schema, &["grok", "defaults", "grokPath"]).is_none());
        assert!(schema_property(&schema, &["openclaw", "defaults", "grokPath"]).is_none());
    }

    #[test]
    fn generated_schema_does_not_accept_null_config_values() {
        let schema = generate_config_schema_json().unwrap();
        let value = serde_json::from_str::<Value>(&schema).unwrap();

        assert!(!schema.contains("\"null\""));
        let limit = schema_property(&value, &["commands", "blocks", "tokenLimit"]).unwrap();
        let types = limit["anyOf"]
            .as_array()
            .unwrap()
            .iter()
            .map(|branch| branch["type"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(types, ["string", "integer"]);
    }

    #[test]
    fn generated_schema_keeps_legacy_root_definition_shape() {
        let schema = generated_schema();

        assert_eq!(
            schema["$ref"].as_str(),
            Some("#/definitions/ccusage-config")
        );
        assert_eq!(
            schema["definitions"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["ccusage-config"]
        );
        assert_properties(
            &schema,
            "ccusage-config",
            &[
                "$schema",
                "amp",
                "claude",
                "cline",
                "codebuff",
                "codex",
                "commands",
                "copilot",
                "defaults",
                "droid",
                "dsh",
                "gemini",
                "antigravity",
                "goose",
                "grok",
                "hermes",
                "kilo",
                "kimi",
                "opencode",
                "openclaw",
                "pi",
                "qwen",
                "zcode",
            ],
        );
        assert!(
            schema["definitions"]["ccusage-config"]["properties"]["defaults"]["properties"]
                .is_object()
        );
    }

    #[test]
    fn schema_allows_cli_config_file_shape() {
        let schema = generated_schema();
        let config = serde_json::json!({
            "$schema": "https://ccusage.com/config-schema.json",
            "defaults": {
                "json": true,
                "compact": true,
                "timezone": "Asia/Tokyo"
            },
            "commands": {
                "daily": {
                    "since": "20260101"
                },
                "weekly": {
                    "startOfWeek": "monday"
                }
            },
            "claude": {
                "commands": {
                    "weekly": {
                        "startOfWeek": "monday"
                    },
                    "blocks": {
                        "active": true,
                        "tokenLimit": "500000",
                        "sessionLength": 6
                    },
                    "statusline": {
                        "visualBurnRate": "emoji-text",
                        "costSource": "both",
                        "refreshInterval": 3
                    }
                }
            },
            "codex": {
                "commands": {
                    "monthly": {
                        "speed": "standard",
                        "since": "20260101"
                    }
                }
            },
            "opencode": {
                "commands": {
                    "weekly": {
                        "json": true
                    }
                }
            },
            "amp": {
                "commands": {
                    "daily": {
                        "breakdown": true
                    }
                }
            },
            "droid": {
                "commands": {
                    "daily": {
                        "json": true
                    }
                }
            },
            "codebuff": {
                "commands": {
                    "daily": {
                        "json": true
                    }
                }
            },
            "pi": {
                "stores": [
                    {
                        "name": "omp",
                        "path": "~/.omp/agent/sessions"
                    }
                ],
                "commands": {
                    "daily": {
                        "piPath": "/tmp/pi-sessions"
                    }
                }
            },
            "goose": {
                "commands": {
                    "daily": {
                        "json": true
                    }
                }
            },
            "openclaw": {
                "commands": {
                    "daily": {
                        "openClawPath": "/tmp/openclaw"
                    }
                }
            },
            "hermes": {
                "commands": {
                    "daily": {
                        "json": true
                    }
                }
            },
            "kilo": {
                "commands": {
                    "daily": {
                        "json": true
                    }
                }
            },
            "copilot": {
                "commands": {
                    "session": {
                        "json": true
                    }
                }
            },
            "gemini": {
                "commands": {
                    "session": {
                        "json": true
                    }
                }
            },
            "kimi": {
                "commands": {
                    "session": {
                        "json": true
                    }
                }
            },
            "qwen": {
                "commands": {
                    "session": {
                        "json": true
                    }
                }
            },
            "cline": {
                "commands": {
                    "weekly": {
                        "json": true
                    }
                }
            }
        });

        assert_value_keys_allowed_by_schema(&config, &schema, &schema);
    }

    #[test]
    fn schema_allows_agent_command_keys_with_the_agent_report_options() {
        let schema = generated_schema();
        let config = serde_json::json!({
            "commands": {
                "daily": { "since": "20260101" },
                "codex daily": { "speed": "fast" },
                "codex:monthly": { "speed": "standard" },
                "codex weekly": { "speed": "fast" },
                "codex:weekly": { "mode": "display" },
                "claude blocks": { "tokenLimit": "500000" },
                "claude:statusline": { "refreshInterval": 3 }
            },
            "codex": { "commands": { "weekly": { "speed": "fast" } } }
        });
        let commands = schema
            .pointer("/definitions/ccusage-config/properties/commands/properties")
            .unwrap();
        let source_keys = ccusage_cli::BUILT_IN_AGENT_NAMES.iter().flat_map(|agent| {
            ["daily", "weekly", "monthly", "session"]
                .into_iter()
                .flat_map(move |report| [format!("{agent} {report}"), format!("{agent}:{report}")])
        });

        assert_value_keys_allowed_by_schema(&config, &schema, &schema);
        for key in source_keys {
            assert!(commands.get(&key).is_some(), "{key}");
        }
        for key in ["codex:blocks", "amp statusline", "unknown daily"] {
            assert!(commands.get(key).is_none(), "{key}");
        }
    }

    #[test]
    fn schema_allows_repository_example_config() {
        let schema = generated_schema();
        let config =
            serde_json::from_str::<Value>(include_str!("../../../../ccusage.example.json"))
                .unwrap();

        assert_value_keys_allowed_by_schema(&config, &schema, &schema);
    }

    #[test]
    fn generated_schema_exposes_cli_defaults() {
        let schema = generated_schema();

        assert_eq!(
            property_default(&schema, &["defaults", "json"]),
            Some(&json!(false))
        );
        assert_eq!(
            property_default(&schema, &["defaults", "mode"]),
            Some(&json!("auto"))
        );
        assert_eq!(
            property_default(&schema, &["defaults", "debugSamples"]),
            Some(&json!(5))
        );
        assert_eq!(
            property_default(&schema, &["defaults", "order"]),
            Some(&json!("asc"))
        );
        assert_eq!(
            property_default(&schema, &["commands", "weekly", "startOfWeek"]),
            Some(&json!("monday"))
        );
        assert_eq!(
            property_default(&schema, &["claude", "commands", "weekly", "startOfWeek"]),
            Some(&json!("sunday"))
        );
        assert_eq!(
            property_default(&schema, &["commands", "blocks", "sessionLength"]),
            Some(&json!(5.0))
        );
        assert_eq!(
            property_default(&schema, &["commands", "statusline", "offline"]),
            Some(&json!(true))
        );
        assert_eq!(
            property_default(&schema, &["commands", "statusline", "visualBurnRate"]),
            Some(&json!("off"))
        );
        assert_eq!(
            property_default(&schema, &["commands", "statusline", "refreshInterval"]),
            Some(&json!(1))
        );
        assert_eq!(
            property_default(&schema, &["codex", "defaults", "speed"]),
            Some(&json!("auto"))
        );
    }

    #[test]
    fn generated_schema_exposes_named_pi_stores() {
        let schema = generated_schema();
        let stores = schema_property(&schema, &["pi", "stores"]).unwrap();

        assert_eq!(stores["type"], json!("array"));
        assert_eq!(
            stores["items"]["properties"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            ["name", "path"].into_iter().collect::<BTreeSet<_>>()
        );
        assert_eq!(
            stores["items"]["properties"]["name"]["pattern"],
            json!("^[a-z][a-z0-9_-]{0,31}$")
        );
    }

    #[test]
    fn statusline_options_parse_model_label_aliases() {
        let map = serde_json::json!({
            "modelLabelAliases": {
                "arn:aws:bedrock:ap-northeast-1:012345678910:application-inference-profile/abcde12345": "claude-opus-4-6"
            }
        });
        let options = serde_json::from_value::<StatuslineSpecificOptions>(map).unwrap();

        let aliases = options.model_label_aliases.unwrap();
        assert_eq!(
            aliases.get(
                "arn:aws:bedrock:ap-northeast-1:012345678910:application-inference-profile/abcde12345"
            ),
            Some(&"claude-opus-4-6".to_string())
        );
    }

    #[test]
    fn statusline_options_reject_non_string_alias_values() {
        let map = serde_json::json!({
            "modelLabelAliases": {
                "valid": "short",
                "invalid": 123
            }
        });
        assert!(serde_json::from_value::<StatuslineSpecificOptions>(map).is_err());
    }

    #[test]
    fn codex_statusline_schema_exposes_shared_options_with_offline_default() {
        let schema: Value = serde_json::from_str(&generate_config_schema_json().unwrap()).unwrap();
        assert_eq!(
            property_default(&schema, &["codex", "commands", "statusline", "offline"]),
            Some(&json!(true))
        );
        assert!(schema_property(&schema, &["codex", "commands", "statusline", "speed"]).is_some());
        for option in ["breakdown", "compact", "order", "debug", "all", "color"] {
            assert!(
                schema_property(&schema, &["codex", "commands", "statusline", option]).is_none(),
                "{option}"
            );
        }
        for name in ["codex statusline", "codex:statusline"] {
            assert!(schema_property(&schema, &["commands", name]).is_some());
        }
    }

    #[test]
    fn snapshots_schema_agent_specific_option_edges() {
        let schema = generated_schema();

        insta::assert_json_snapshot!(json!({
            "rootRef": schema["$ref"],
            "rootProperties": definition_properties(&schema, "ccusage-config"),
            "rootAdditionalProperties": schema["definitions"]["ccusage-config"]["additionalProperties"],
            "defaults": schema_node(&schema, &["defaults"]),
            "rootDaily": schema_node(&schema, &["commands", "daily"]),
            "claudeStatusline": schema_node(&schema, &["claude", "commands", "statusline"]),
            "codexDefaults": schema_node(&schema, &["codex", "defaults"]),
            "opencodeWeekly": schema_node(&schema, &["opencode", "commands", "weekly"]),
            "piDefaults": schema_node(&schema, &["pi", "defaults"]),
            "openclawDefaults": schema_node(&schema, &["openclaw", "defaults"]),
            "grokDefaults": schema_node(&schema, &["grok", "defaults"]),
            "zcodeDefaults": schema_node(&schema, &["zcode", "defaults"]),
            "clineWeekly": schema_node(&schema, &["cline", "commands", "weekly"]),
        }));
    }

    #[test]
    fn token_limit_schema_requires_max_or_positive_integers() {
        let schema = generated_schema();
        let token_limit = schema_property(&schema, &["commands", "blocks", "tokenLimit"]).unwrap();
        assert_eq!(token_limit["anyOf"][1]["minimum"], 1.0);
        assert!(token_limit["anyOf"][0]["pattern"].as_str().is_some());
    }

    fn generated_schema() -> Value {
        serde_json::from_str(&generate_config_schema_json().unwrap()).unwrap()
    }

    fn assert_properties(schema: &Value, definition: &str, expected: &[&str]) {
        assert_eq!(
            definition_properties(schema, definition),
            expected.iter().copied().collect::<BTreeSet<_>>(),
            "{definition} properties did not match"
        );
    }

    fn definition_properties<'a>(schema: &'a Value, definition: &str) -> BTreeSet<&'a str> {
        schema["definitions"][definition]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect()
    }

    fn assert_schema_properties(schema: &Value, path: &[&str], expected: &[&str]) {
        assert_eq!(
            schema_properties(schema, path),
            expected.iter().copied().collect::<BTreeSet<_>>(),
            "{path:?} properties did not match"
        );
    }

    fn schema_properties<'a>(schema: &'a Value, path: &[&str]) -> BTreeSet<&'a str> {
        schema_node(schema, path)["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect()
    }

    fn property_default<'a>(schema: &'a Value, path: &[&str]) -> Option<&'a Value> {
        schema_property(schema, path).and_then(|property| property.get("default"))
    }

    fn schema_property<'a>(schema: &'a Value, path: &[&str]) -> Option<&'a Value> {
        let (property, parent_path) = path.split_last().unwrap();
        schema_node(schema, parent_path)["properties"].get(*property)
    }

    fn schema_node<'a>(schema: &'a Value, path: &[&str]) -> &'a Value {
        let mut node = &schema["definitions"]["ccusage-config"];
        for segment in path {
            node = &node["properties"][*segment];
        }
        node
    }

    fn with_keys<'a>(base: &[&'a str], extra: &[&'a str]) -> Vec<&'a str> {
        base.iter().chain(extra).copied().collect()
    }

    fn assert_value_keys_allowed_by_schema(value: &Value, schema: &Value, root: &Value) {
        let Some(value_object) = value.as_object() else {
            return;
        };
        let schema = resolve_schema(schema, root);
        let schema = merge_all_of(schema, root);
        let properties = schema.get("properties").and_then(Value::as_object);
        for (key, child_value) in value_object {
            let child_schema = if let Some(properties) = properties {
                if let Some(child_schema) = properties.get(key) {
                    child_schema
                } else if schema
                    .get("additionalProperties")
                    .is_some_and(|v| !v.is_null() && *v != Value::Bool(false))
                {
                    schema.get("additionalProperties").unwrap()
                } else {
                    panic!("schema does not allow config key {key}");
                }
            } else if schema
                .get("additionalProperties")
                .is_some_and(|v| !v.is_null() && *v != Value::Bool(false))
            {
                schema.get("additionalProperties").unwrap()
            } else {
                panic!("schema node has no properties: {schema:?}");
            };
            assert_value_keys_allowed_by_schema(child_value, child_schema, root);
        }
    }

    fn resolve_schema<'a>(schema: &'a Value, root: &'a Value) -> &'a Value {
        let Some(reference) = schema.get("$ref").and_then(Value::as_str) else {
            return schema;
        };
        root.pointer(reference.strip_prefix('#').unwrap()).unwrap()
    }

    fn merge_all_of(schema: &Value, root: &Value) -> Value {
        let Some(items) = schema.get("allOf").and_then(Value::as_array) else {
            return schema.clone();
        };
        let mut merged = schema.clone();
        let properties = merged
            .as_object_mut()
            .unwrap()
            .entry("properties")
            .or_insert_with(|| Value::Object(Default::default()));
        let properties = properties.as_object_mut().unwrap();
        for item in items {
            let resolved = resolve_schema(item, root);
            for (key, value) in resolved["properties"].as_object().unwrap() {
                properties.insert(key.clone(), value.clone());
            }
        }
        merged
    }
}
