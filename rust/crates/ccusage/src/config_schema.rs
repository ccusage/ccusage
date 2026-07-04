#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use schemars::{JsonSchema, r#gen::SchemaSettings};

pub(crate) const NAMED_PI_STORE_NAME_PATTERN: &str = "^[a-z][a-z0-9_-]{0,31}$";

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CcusageConfig {
    /// JSON Schema URL for validation and autocomplete.
    #[serde(rename = "$schema")]
    pub(crate) schema_url: Option<String>,
    /// Default values for all-agent reports and legacy Claude commands.
    pub(crate) defaults: Option<SharedOptions>,
    /// Command-specific configuration for all-agent reports.
    pub(crate) commands: Option<RootCommandsConfig>,
    /// Claude Code configuration.
    pub(crate) claude: Option<ClaudeConfig>,
    /// Codex configuration.
    pub(crate) codex: Option<CodexConfig>,
    /// OpenCode configuration.
    pub(crate) opencode: Option<OpenCodeConfig>,
    /// Amp configuration.
    pub(crate) amp: Option<AmpConfig>,
    /// Droid configuration.
    pub(crate) droid: Option<DroidConfig>,
    /// Codebuff configuration.
    pub(crate) codebuff: Option<CodebuffConfig>,
    /// Hermes Agent configuration.
    pub(crate) hermes: Option<HermesConfig>,
    /// pi-agent configuration.
    pub(crate) pi: Option<PiConfig>,
    /// Goose configuration.
    pub(crate) goose: Option<GooseConfig>,
    /// OpenClaw configuration.
    pub(crate) openclaw: Option<OpenClawConfig>,
    /// Kilo configuration.
    pub(crate) kilo: Option<KiloConfig>,
    /// GitHub Copilot CLI configuration.
    pub(crate) copilot: Option<CopilotConfig>,
    /// Gemini CLI configuration.
    pub(crate) gemini: Option<GeminiConfig>,
    /// Kimi configuration.
    pub(crate) kimi: Option<KimiConfig>,
    /// Qwen configuration.
    pub(crate) qwen: Option<QwenConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RootCommandsConfig {
    pub(crate) daily: Option<DailyOptions>,
    pub(crate) weekly: Option<WeeklyOptions>,
    pub(crate) monthly: Option<SharedOptions>,
    pub(crate) session: Option<SharedOptions>,
    pub(crate) blocks: Option<BlocksOptions>,
    pub(crate) statusline: Option<StatuslineOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClaudeConfig {
    pub(crate) defaults: Option<ClaudeOptions>,
    pub(crate) commands: Option<ClaudeCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClaudeCommandsConfig {
    pub(crate) daily: Option<DailyOptions>,
    pub(crate) weekly: Option<WeeklyOptions>,
    pub(crate) monthly: Option<SharedOptions>,
    pub(crate) session: Option<SharedOptions>,
    pub(crate) blocks: Option<BlocksOptions>,
    pub(crate) statusline: Option<StatuslineOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CodexConfig {
    pub(crate) defaults: Option<CodexOptions>,
    pub(crate) commands: Option<CodexCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CodexCommandsConfig {
    pub(crate) daily: Option<CodexOptions>,
    pub(crate) monthly: Option<CodexOptions>,
    pub(crate) session: Option<CodexOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenCodeConfig {
    pub(crate) defaults: Option<SharedOptions>,
    pub(crate) commands: Option<OpenCodeCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenCodeCommandsConfig {
    pub(crate) daily: Option<SharedOptions>,
    pub(crate) weekly: Option<SharedOptions>,
    pub(crate) monthly: Option<SharedOptions>,
    pub(crate) session: Option<SharedOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AmpConfig {
    pub(crate) defaults: Option<SharedOptions>,
    pub(crate) commands: Option<AmpCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AmpCommandsConfig {
    pub(crate) daily: Option<SharedOptions>,
    pub(crate) monthly: Option<SharedOptions>,
    pub(crate) session: Option<SharedOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DroidConfig {
    pub(crate) defaults: Option<SharedOptions>,
    pub(crate) commands: Option<DroidCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DroidCommandsConfig {
    pub(crate) daily: Option<SharedOptions>,
    pub(crate) monthly: Option<SharedOptions>,
    pub(crate) session: Option<SharedOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CodebuffConfig {
    pub(crate) defaults: Option<SharedOptions>,
    pub(crate) commands: Option<CodebuffCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CodebuffCommandsConfig {
    pub(crate) daily: Option<SharedOptions>,
    pub(crate) monthly: Option<SharedOptions>,
    pub(crate) session: Option<SharedOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HermesConfig {
    pub(crate) defaults: Option<SharedOptions>,
    pub(crate) commands: Option<HermesCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HermesCommandsConfig {
    pub(crate) daily: Option<SharedOptions>,
    pub(crate) monthly: Option<SharedOptions>,
    pub(crate) session: Option<SharedOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PiConfig {
    /// Additional named pi-format session stores included in all-agent reports.
    pub(crate) stores: Option<Vec<PiStoreConfig>>,
    pub(crate) defaults: Option<PiOptions>,
    pub(crate) commands: Option<PiCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PiStoreConfig {
    /// Agent name to use for this pi-format store in all-agent reports.
    #[schemars(regex(pattern = "^[a-z][a-z0-9_-]{0,31}$"))]
    pub(crate) name: String,
    /// Path to this pi-format sessions directory. Comma-separated lists are supported.
    pub(crate) path: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PiCommandsConfig {
    pub(crate) daily: Option<PiOptions>,
    pub(crate) monthly: Option<PiOptions>,
    pub(crate) session: Option<PiOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GooseConfig {
    pub(crate) defaults: Option<SharedOptions>,
    pub(crate) commands: Option<GooseCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GooseCommandsConfig {
    pub(crate) daily: Option<SharedOptions>,
    pub(crate) monthly: Option<SharedOptions>,
    pub(crate) session: Option<SharedOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenClawConfig {
    pub(crate) defaults: Option<OpenClawOptions>,
    pub(crate) commands: Option<OpenClawCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenClawCommandsConfig {
    pub(crate) daily: Option<OpenClawOptions>,
    pub(crate) monthly: Option<OpenClawOptions>,
    pub(crate) session: Option<OpenClawOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KiloConfig {
    pub(crate) defaults: Option<SharedOptions>,
    pub(crate) commands: Option<KiloCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KiloCommandsConfig {
    pub(crate) daily: Option<SharedOptions>,
    pub(crate) monthly: Option<SharedOptions>,
    pub(crate) session: Option<SharedOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CopilotConfig {
    pub(crate) defaults: Option<SharedOptions>,
    pub(crate) commands: Option<CopilotCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CopilotCommandsConfig {
    pub(crate) daily: Option<SharedOptions>,
    pub(crate) monthly: Option<SharedOptions>,
    pub(crate) session: Option<SharedOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GeminiConfig {
    pub(crate) defaults: Option<SharedOptions>,
    pub(crate) commands: Option<GeminiCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GeminiCommandsConfig {
    pub(crate) daily: Option<SharedOptions>,
    pub(crate) monthly: Option<SharedOptions>,
    pub(crate) session: Option<SharedOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KimiConfig {
    pub(crate) defaults: Option<SharedOptions>,
    pub(crate) commands: Option<KimiCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KimiCommandsConfig {
    pub(crate) daily: Option<SharedOptions>,
    pub(crate) monthly: Option<SharedOptions>,
    pub(crate) session: Option<SharedOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QwenConfig {
    pub(crate) defaults: Option<SharedOptions>,
    pub(crate) commands: Option<QwenCommandsConfig>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QwenCommandsConfig {
    pub(crate) daily: Option<SharedOptions>,
    pub(crate) monthly: Option<SharedOptions>,
    pub(crate) session: Option<SharedOptions>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SharedOptions {
    /// Filter from date (YYYY-MM-DD or YYYYMMDD).
    pub(crate) since: Option<String>,
    /// Filter until date (inclusive).
    pub(crate) until: Option<String>,
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
    pub(crate) all: Option<bool>,
    /// Force compact table layout for narrow terminals.
    pub(crate) compact: Option<bool>,
    /// Disable parallel file processing.
    pub(crate) single_thread: Option<bool>,
    /// Hide cost information in table and JSON output.
    pub(crate) no_cost: Option<bool>,
    /// Runtime pricing overrides keyed by raw model name.
    pub(crate) pricing_overrides: Option<BTreeMap<String, ConfigPricingOverride>>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClaudeOptions {
    #[serde(flatten)]
    pub(crate) shared: SharedOptions,
    #[serde(flatten)]
    pub(crate) daily: DailySpecificOptions,
    #[serde(flatten)]
    pub(crate) weekly: WeeklySpecificOptions,
    #[serde(flatten)]
    pub(crate) blocks: BlocksSpecificOptions,
    #[serde(flatten)]
    pub(crate) statusline: StatuslineSpecificOptions,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DailyOptions {
    #[serde(flatten)]
    pub(crate) shared: SharedOptions,
    #[serde(flatten)]
    pub(crate) daily: DailySpecificOptions,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DailySpecificOptions {
    /// Show per-session project instances.
    pub(crate) instances: Option<bool>,
    /// Filter to a project name or path.
    pub(crate) project: Option<String>,
    /// JSON object or path mapping project paths to display aliases.
    pub(crate) project_aliases: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WeeklyOptions {
    #[serde(flatten)]
    pub(crate) shared: SharedOptions,
    #[serde(flatten)]
    pub(crate) weekly: WeeklySpecificOptions,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WeeklySpecificOptions {
    /// First day of week for weekly grouping.
    pub(crate) start_of_week: Option<ConfigWeekDay>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BlocksOptions {
    #[serde(flatten)]
    pub(crate) shared: SharedOptions,
    #[serde(flatten)]
    pub(crate) blocks: BlocksSpecificOptions,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BlocksSpecificOptions {
    /// Show the active session block.
    pub(crate) active: Option<bool>,
    /// Show recent session blocks.
    pub(crate) recent: Option<bool>,
    /// Token limit for session block calculations.
    pub(crate) token_limit: Option<String>,
    /// Session block duration in hours.
    pub(crate) session_length: Option<f64>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatuslineOptions {
    #[serde(flatten)]
    pub(crate) statusline: StatuslineSpecificOptions,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
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
    pub(crate) context_low_threshold: Option<u64>,
    /// Percentage threshold for medium context warning.
    pub(crate) context_medium_threshold: Option<u64>,
    /// Timezone for date grouping (IANA).
    pub(crate) timezone: Option<String>,
    /// Show statusline debug information.
    pub(crate) debug: Option<bool>,
    /// Map model identifiers to short display labels.
    pub(crate) model_label_aliases: Option<HashMap<String, String>>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CodexOptions {
    #[serde(flatten)]
    pub(crate) shared: SharedOptions,
    /// Codex speed normalization strategy.
    pub(crate) speed: Option<ConfigCodexSpeed>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PiOptions {
    #[serde(flatten)]
    pub(crate) shared: SharedOptions,
    /// Path or comma-separated paths to pi-agent sessions directories.
    pub(crate) pi_path: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenClawOptions {
    #[serde(flatten)]
    pub(crate) shared: SharedOptions,
    /// Path or comma-separated paths to OpenClaw data directories.
    pub(crate) open_claw_path: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ConfigCostMode {
    Auto,
    Calculate,
    Display,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ConfigSortOrder {
    Desc,
    Asc,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ConfigWeekDay {
    Sunday,
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ConfigCodexSpeed {
    Auto,
    Standard,
    Fast,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ConfigVisualBurnRate {
    Off,
    Emoji,
    Text,
    EmojiText,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ConfigCostSource {
    Auto,
    Ccusage,
    Cc,
    Both,
}

#[derive(Debug, Default, Deserialize, JsonSchema, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfigPricingOverride {
    pub(crate) input_cost_per_token: Option<f64>,
    pub(crate) output_cost_per_token: Option<f64>,
    pub(crate) cache_creation_input_token_cost: Option<f64>,
    pub(crate) cache_read_input_token_cost: Option<f64>,
    pub(crate) input_cost_per_token_above_200k_tokens: Option<f64>,
    pub(crate) output_cost_per_token_above_200k_tokens: Option<f64>,
    pub(crate) cache_creation_input_token_cost_above_200k_tokens: Option<f64>,
    pub(crate) cache_read_input_token_cost_above_200k_tokens: Option<f64>,
    pub(crate) max_input_tokens: Option<u64>,
    pub(crate) fast_multiplier: Option<f64>,
}

impl SharedOptions {
    pub(crate) fn from_map(map: &Map<String, Value>) -> Self {
        Self {
            since: string_option(map, "since"),
            until: string_option(map, "until"),
            json: bool_option(map, "json"),
            mode: enum_option(map, "mode"),
            debug: bool_option(map, "debug"),
            debug_samples: usize_option(map, "debugSamples"),
            order: enum_option(map, "order"),
            breakdown: bool_option(map, "breakdown"),
            offline: bool_option(map, "offline"),
            no_offline: bool_option(map, "noOffline"),
            color: bool_option(map, "color"),
            no_color: bool_option(map, "noColor"),
            timezone: string_option(map, "timezone"),
            jq: string_option(map, "jq"),
            all: bool_option(map, "all"),
            compact: bool_option(map, "compact"),
            single_thread: bool_option(map, "singleThread"),
            no_cost: bool_option(map, "noCost"),
            pricing_overrides: pricing_override_map_option(map, "pricingOverrides"),
        }
    }
}

impl DailySpecificOptions {
    pub(crate) fn from_map(map: &Map<String, Value>) -> Self {
        Self {
            instances: bool_option(map, "instances"),
            project: string_option(map, "project"),
            project_aliases: string_option(map, "projectAliases"),
        }
    }
}

impl WeeklySpecificOptions {
    pub(crate) fn from_map(map: &Map<String, Value>) -> Self {
        Self {
            start_of_week: enum_option(map, "startOfWeek"),
        }
    }
}

impl BlocksSpecificOptions {
    pub(crate) fn from_map(map: &Map<String, Value>) -> Self {
        Self {
            active: bool_option(map, "active"),
            recent: bool_option(map, "recent"),
            token_limit: string_option(map, "tokenLimit"),
            session_length: f64_option(map, "sessionLength"),
        }
    }
}

impl StatuslineSpecificOptions {
    pub(crate) fn from_map(map: &Map<String, Value>) -> Self {
        Self {
            offline: bool_option(map, "offline"),
            no_offline: bool_option(map, "noOffline"),
            visual_burn_rate: enum_option(map, "visualBurnRate"),
            cost_source: enum_option(map, "costSource"),
            cache: bool_option(map, "cache"),
            no_cache: bool_option(map, "noCache"),
            refresh_interval: u64_option(map, "refreshInterval"),
            context_low_threshold: u64_option(map, "contextLowThreshold"),
            context_medium_threshold: u64_option(map, "contextMediumThreshold"),
            timezone: string_option(map, "timezone"),
            debug: bool_option(map, "debug"),
            model_label_aliases: hashmap_option(map, "modelLabelAliases"),
        }
    }
}

impl CodexOptions {
    pub(crate) fn from_map(map: &Map<String, Value>) -> Self {
        Self {
            shared: SharedOptions::from_map(map),
            speed: enum_option(map, "speed"),
        }
    }
}

impl PiOptions {
    pub(crate) fn from_map(map: &Map<String, Value>) -> Self {
        Self {
            shared: SharedOptions::from_map(map),
            pi_path: string_option(map, "piPath"),
        }
    }
}

impl OpenClawOptions {
    pub(crate) fn from_map(map: &Map<String, Value>) -> Self {
        Self {
            shared: SharedOptions::from_map(map),
            open_claw_path: string_option(map, "openClawPath"),
        }
    }
}

pub(crate) fn generate_config_schema_json() -> String {
    let generator = SchemaSettings::draft07()
        .with(|settings| {
            settings.meta_schema = Some("https://json-schema.org/draft-07/schema#".to_string());
            settings.option_add_null_type = false;
        })
        .into_generator();
    let mut schema =
        serde_json::to_value(generator.into_root_schema_for::<CcusageConfig>()).unwrap();
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
            json!([
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
            ]),
        );
    }
    enrich_schema(&mut schema);
    add_schema_defaults(&mut schema);
    inline_schema_references(&mut schema);
    wrap_root_schema(&mut schema);
    let mut json = tab_indent_json(&serde_json::to_string_pretty(&schema).unwrap());
    json.push('\n');
    json
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

fn string_option(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key)?.as_str().map(ToString::to_string)
}

fn bool_option(map: &Map<String, Value>, key: &str) -> Option<bool> {
    map.get(key)?.as_bool()
}

fn usize_option(map: &Map<String, Value>, key: &str) -> Option<usize> {
    map.get(key)?
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
}

fn u64_option(map: &Map<String, Value>, key: &str) -> Option<u64> {
    map.get(key)?.as_u64()
}

fn f64_option(map: &Map<String, Value>, key: &str) -> Option<f64> {
    map.get(key)?.as_f64()
}

fn enum_option<T>(map: &Map<String, Value>, key: &str) -> Option<T>
where
    T: DeserializeOwned,
{
    serde_json::from_value(map.get(key)?.clone()).ok()
}

fn pricing_override_map_option(
    map: &Map<String, Value>,
    key: &str,
) -> Option<BTreeMap<String, ConfigPricingOverride>> {
    serde_json::from_value(map.get(key)?.clone()).ok()
}

fn hashmap_option(map: &Map<String, Value>, key: &str) -> Option<HashMap<String, String>> {
    let obj = map.get(key)?.as_object()?;
    let mut result = HashMap::new();
    for (k, v) in obj {
        if let Some(s) = v.as_str() {
            result.insert(k.clone(), s.to_string());
        }
    }
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
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
    set_definition_defaults(schema, "WeeklyOptions", &[("startOfWeek", json!("sunday"))]);
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
        assert!(schema_property(&schema, &["kimi", "defaults", "openClawPath"]).is_none());
        assert!(schema_property(&schema, &["qwen", "defaults", "openClawPath"]).is_none());
    }

    #[test]
    fn generated_schema_does_not_accept_null_config_values() {
        let schema = generate_config_schema_json();
        let value = serde_json::from_str::<Value>(&schema).unwrap();

        assert!(!schema.contains("\"null\""));
        assert!(!contains_key(&value, "anyOf"));
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
                "$schema", "amp", "claude", "codebuff", "codex", "commands", "copilot", "defaults",
                "gemini", "goose", "hermes", "kilo", "kimi", "opencode", "openclaw", "pi", "qwen",
                "droid",
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
            }
        });

        assert_value_keys_allowed_by_schema(&config, &schema, &schema);
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
        let options = StatuslineSpecificOptions::from_map(map.as_object().unwrap());

        let aliases = options.model_label_aliases.unwrap();
        assert_eq!(
            aliases.get(
                "arn:aws:bedrock:ap-northeast-1:012345678910:application-inference-profile/abcde12345"
            ),
            Some(&"claude-opus-4-6".to_string())
        );
    }

    #[test]
    fn statusline_options_ignore_non_string_alias_values() {
        let map = serde_json::json!({
            "modelLabelAliases": {
                "valid": "short",
                "invalid": 123
            }
        });
        let options = StatuslineSpecificOptions::from_map(map.as_object().unwrap());

        let aliases = options.model_label_aliases.unwrap();
        assert_eq!(aliases.get("valid"), Some(&"short".to_string()));
        assert!(!aliases.contains_key("invalid"));
    }

    #[test]
    fn snapshots_schema_agent_specific_option_edges() {
        if running_in_schema_generator_test_binary() {
            return;
        }
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
        }));
    }

    fn running_in_schema_generator_test_binary() -> bool {
        std::env::current_exe()
            .ok()
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .is_some_and(|name| {
                name.starts_with("generate_config_schema")
                    || name.starts_with("generate-config-schema")
            })
    }

    fn generated_schema() -> Value {
        serde_json::from_str(&generate_config_schema_json()).unwrap()
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

    fn contains_key(value: &Value, key: &str) -> bool {
        match value {
            Value::Object(map) => {
                map.contains_key(key) || map.values().any(|value| contains_key(value, key))
            }
            Value::Array(values) => values.iter().any(|value| contains_key(value, key)),
            _ => false,
        }
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
        let definition = reference.strip_prefix("#/definitions/").unwrap();
        &root["definitions"][definition]
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
