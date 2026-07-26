use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
};

use serde_json::{Map, Value};

use crate::{
    cli::{
        BlocksArgs, CodexSpeed, CostMode, CostSource, DailyArgs, NamedPiStore, PricingOverride,
        SharedArgs, SortOrder, StatuslineArgs, VisualBurnRate, WeekDay, WeeklyArgs,
        normalize_date_bound,
    },
    config_schema::{
        BlocksSpecificOptions, CodexOptions, ConfigCodexSpeed, ConfigCostMode, ConfigCostSource,
        ConfigPricingOverride, ConfigSortOrder, ConfigVisualBurnRate, ConfigWeekDay,
        DailySpecificOptions, NAMED_PI_STORE_NAME_PATTERN, OpenClawOptions, PiOptions,
        SharedOptions, StatuslineSpecificOptions, WeeklySpecificOptions,
    },
};

struct ConfigCommand {
    raw: String,
    agent: Option<String>,
    report: String,
}

pub(crate) struct ConfigContext {
    value: Option<Value>,
    command: ConfigCommand,
    pi_stores: Vec<NamedPiStore>,
    error: Option<String>,
}

impl ConfigContext {
    pub(crate) fn from_args(args: &[String]) -> Self {
        let command = detect_config_command(args);
        let value = load_config_value(scan_config_path(args).as_deref());
        let (pi_stores, error) = value
            .as_ref()
            .map(parse_named_pi_stores)
            .transpose()
            .map(|stores| (stores.unwrap_or_default(), None))
            .unwrap_or_else(|error| (Vec::new(), Some(error)));
        Self {
            value,
            command,
            pi_stores,
            error,
        }
    }

    fn option_maps(&self) -> Vec<&Map<String, Value>> {
        let mut maps = Vec::new();
        let Some(root) = self.value.as_ref().and_then(Value::as_object) else {
            return maps;
        };
        if let Some(defaults) = object_at(root, "defaults") {
            maps.push(defaults);
        }
        if let Some(commands) = object_at(root, "commands") {
            if let Some(raw) = object_at(commands, &self.command.raw) {
                maps.push(raw);
            }
            if self.command.agent.is_some() {
                if let Some(report) = object_at(commands, &self.command.report) {
                    maps.push(report);
                }
                if let Some(agent) = self.command.agent.as_deref() {
                    let colon_name = format!("{agent}:{}", self.command.report);
                    if let Some(agent_report) = object_at(commands, &colon_name) {
                        maps.push(agent_report);
                    }
                }
            }
        }
        if let Some(agent) = self
            .command
            .agent
            .as_deref()
            .and_then(|agent| object_at(root, agent))
        {
            if let Some(defaults) = object_at(agent, "defaults") {
                maps.push(defaults);
            }
            if let Some(command) = object_at(agent, "commands")
                .and_then(|commands| object_at(commands, &self.command.report))
            {
                maps.push(command);
            }
        }
        maps
    }

    fn command_uses_named_pi_stores(&self) -> bool {
        self.command.agent.is_none()
            && matches!(
                self.command.report.as_str(),
                "daily" | "weekly" | "monthly" | "session"
            )
    }
}

fn parse_named_pi_stores(value: &Value) -> std::result::Result<Vec<NamedPiStore>, String> {
    let Some(stores_value) = value
        .as_object()
        .and_then(|root| root.get("pi"))
        .and_then(Value::as_object)
        .and_then(|pi| pi.get("stores"))
    else {
        return Ok(Vec::new());
    };
    let Some(stores) = stores_value.as_array() else {
        return Err(config_error("pi.stores must be an array"));
    };

    let mut names = BTreeSet::new();
    let mut parsed = Vec::with_capacity(stores.len());
    for (index, store) in stores.iter().enumerate() {
        let Some(store) = store.as_object() else {
            return Err(config_error(format!(
                "pi.stores[{index}] must contain string fields 'name' and 'path'"
            )));
        };
        let Some(name) = store.get("name").and_then(Value::as_str) else {
            return Err(config_error(format!(
                "pi.stores[{index}] must contain string fields 'name' and 'path'"
            )));
        };
        let Some(path) = store.get("path").and_then(Value::as_str) else {
            return Err(config_error(format!(
                "pi.stores[{index}] must contain string fields 'name' and 'path'"
            )));
        };
        if path.trim().is_empty() {
            return Err(config_error(format!(
                "pi.stores[{index}] ('{name}'): path must be a non-empty string"
            )));
        }
        validate_named_pi_store_name(name).map_err(|error| match error {
            NamedPiStoreNameError::Pattern => config_error(format!(
                "pi.stores[{index}].name must match {NAMED_PI_STORE_NAME_PATTERN}"
            )),
            NamedPiStoreNameError::Reserved => config_error(format!(
                "pi.stores name '{name}' collides with a built-in agent"
            )),
        })?;
        if !names.insert(name.to_string()) {
            return Err(config_error(format!("duplicate pi.stores name '{name}'")));
        }
        parsed.push(NamedPiStore {
            name: name.to_string(),
            path: path.to_string(),
        });
    }
    Ok(parsed)
}

fn config_error(message: impl Into<String>) -> String {
    format!("Invalid ccusage config: {}", message.into())
}

#[derive(Debug, Eq, PartialEq)]
enum NamedPiStoreNameError {
    Pattern,
    Reserved,
}

fn validate_named_pi_store_name(name: &str) -> std::result::Result<(), NamedPiStoreNameError> {
    if !matches_named_pi_store_name_pattern(name) {
        return Err(NamedPiStoreNameError::Pattern);
    }
    if reserved_named_pi_store_names().contains(&name) {
        return Err(NamedPiStoreNameError::Reserved);
    }
    Ok(())
}

fn reserved_named_pi_store_names() -> Vec<&'static str> {
    std::iter::once("all")
        .chain(crate::adapter::all::BUILT_IN_AGENT_NAMES.iter().copied())
        .collect()
}

fn matches_named_pi_store_name_pattern(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    let rest_len = chars
        .by_ref()
        .try_fold(0usize, |count, ch| {
            (ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '_' | '-'))
                .then_some(count + 1)
        })
        .unwrap_or(usize::MAX);
    rest_len <= 31
}

fn object_at<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a Map<String, Value>> {
    object.get(key).and_then(Value::as_object)
}

fn load_config_value(path: Option<&Path>) -> Option<Value> {
    let paths = match path {
        Some(path) => vec![path.to_path_buf()],
        None => discover_config_paths(),
    };
    paths
        .into_iter()
        .filter_map(|path| fs::read_to_string(path).ok())
        .filter_map(|content| serde_json::from_str::<Value>(&content).ok())
        .find(|value| value.as_object().is_some())
}

fn discover_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(cwd) = env::current_dir() {
        paths.push(cwd.join(".ccusage").join("ccusage.json"));
    }
    paths.extend(
        claude_config_dirs()
            .into_iter()
            .map(|dir| dir.join("ccusage.json")),
    );
    paths
}

fn claude_config_dirs() -> Vec<PathBuf> {
    if let Ok(paths) = env::var("CLAUDE_CONFIG_DIR") {
        return paths
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .collect();
    }
    crate::home::home_dir()
        .map(|home| vec![home.join(".config").join("claude"), home.join(".claude")])
        .unwrap_or_default()
}

fn scan_config_path(args: &[String]) -> Option<PathBuf> {
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if let Some((flag, value)) = arg.split_once('=') {
            if flag == "--config" && !value.is_empty() {
                return Some(PathBuf::from(value));
            }
        } else if arg == "--config" {
            return args.get(index + 1).map(PathBuf::from);
        }
        index += 1;
    }
    None
}

fn detect_config_command(args: &[String]) -> ConfigCommand {
    let tokens = command_tokens(args);
    let Some(first) = tokens.first() else {
        return ConfigCommand {
            raw: "daily".to_string(),
            agent: None,
            report: "daily".to_string(),
        };
    };
    if let Some((agent, report)) = first.split_once(':') {
        return ConfigCommand {
            raw: format!("{agent} {report}"),
            agent: Some(agent.to_string()),
            report: report.to_string(),
        };
    }
    if is_agent_command(first) {
        let report = tokens
            .get(1)
            .filter(|token| is_report_command(token))
            .cloned()
            .unwrap_or_else(|| "daily".to_string());
        return ConfigCommand {
            raw: format!("{first} {report}"),
            agent: Some(first.clone()),
            report,
        };
    }
    ConfigCommand {
        raw: first.clone(),
        agent: None,
        report: first.clone(),
    }
}

fn command_tokens(args: &[String]) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if let Some((flag, _)) = arg.split_once('=')
            && flag.starts_with('-')
        {
            index += 1;
            continue;
        }
        if arg.starts_with('-') {
            index += if option_takes_value(arg) { 2 } else { 1 };
            continue;
        }
        tokens.push(arg.clone());
        index += 1;
    }
    tokens
}

fn option_takes_value(arg: &str) -> bool {
    matches!(
        arg.split_once('=').map_or(arg, |(name, _)| name),
        "-s" | "--since"
            | "-u"
            | "--until"
            | "-m"
            | "--mode"
            | "--debug-samples"
            | "-o"
            | "--order"
            | "-z"
            | "--timezone"
            | "-q"
            | "--jq"
            | "--config"
            | "-t"
            | "--token-limit"
            | "-n"
            | "--session-length"
            | "-w"
            | "--start-of-week"
            | "-p"
            | "--project"
            | "--project-aliases"
            | "--pi-path"
            | "--speed"
            | "-B"
            | "--visual-burn-rate"
            | "--cost-source"
            | "--refresh-interval"
            | "--context-low-threshold"
            | "--context-medium-threshold"
    )
}

fn is_agent_command(command: &str) -> bool {
    crate::adapter::all::BUILT_IN_AGENT_NAMES.contains(&command)
}

fn is_report_command(command: &str) -> bool {
    matches!(
        command,
        "daily" | "monthly" | "weekly" | "session" | "blocks" | "statusline"
    )
}

pub(crate) fn apply_config_to_shared(shared: &mut SharedArgs, config: &ConfigContext) {
    for options in config.option_maps() {
        apply_shared_options(shared, SharedOptions::from_map(options));
    }
    if config.error.is_none() {
        shared.pi_stores = config.pi_stores.clone();
    }
}

pub(crate) fn apply_config_to_daily_args(args: &mut DailyArgs, config: &ConfigContext) {
    for options in config.option_maps() {
        let options = DailySpecificOptions::from_map(options);
        if let Some(instances) = options.instances {
            args.instances = instances;
        }
        if let Some(project) = options.project {
            args.project = Some(project);
        }
        if let Some(project_aliases) = options.project_aliases {
            args.project_aliases = Some(project_aliases);
        }
    }
}

pub(crate) fn apply_config_to_weekly_args(args: &mut WeeklyArgs, config: &ConfigContext) {
    for options in config.option_maps() {
        if let Some(day) = WeeklySpecificOptions::from_map(options).start_of_week {
            args.start_of_week = day.into();
        }
    }
}

pub(crate) fn apply_config_to_blocks_args(args: &mut BlocksArgs, config: &ConfigContext) {
    for options in config.option_maps() {
        let options = BlocksSpecificOptions::from_map(options);
        if let Some(active) = options.active {
            args.active = active;
        }
        if let Some(recent) = options.recent {
            args.recent = recent;
        }
        if let Some(token_limit) = options.token_limit {
            args.token_limit = Some(token_limit);
        }
        if let Some(session_length) = options.session_length {
            args.session_length = session_length;
        }
    }
}

pub(crate) fn apply_config_to_statusline_args(args: &mut StatuslineArgs, config: &ConfigContext) {
    for options in config.option_maps() {
        let options = StatuslineSpecificOptions::from_map(options);
        if let Some(offline) = options.offline {
            args.offline = offline;
        }
        if let Some(no_offline) = options.no_offline {
            args.no_offline = no_offline;
        }
        if let Some(visual_burn_rate) = options.visual_burn_rate {
            args.visual_burn_rate = visual_burn_rate.into();
        }
        if let Some(cost_source) = options.cost_source {
            args.cost_source = cost_source.into();
        }
        if let Some(cache) = options.cache {
            args.cache = cache;
        }
        if let Some(no_cache) = options.no_cache {
            args.no_cache = no_cache;
        }
        if let Some(refresh_interval) = options.refresh_interval {
            args.refresh_interval = refresh_interval;
        }
        if let Some(threshold) = options
            .context_low_threshold
            .and_then(|value| u8::try_from(value).ok())
        {
            args.context_low_threshold = threshold;
        }
        if let Some(threshold) = options
            .context_medium_threshold
            .and_then(|value| u8::try_from(value).ok())
        {
            args.context_medium_threshold = threshold;
        }
        if let Some(timezone) = options.timezone {
            args.timezone = Some(timezone);
        }
        if let Some(debug) = options.debug {
            args.debug = debug;
        }
        if let Some(aliases) = options.model_label_aliases {
            args.model_label_aliases = aliases;
        }
    }
}

pub(crate) fn apply_config_to_agent_args(
    codex_speed: &mut CodexSpeed,
    mut pi_path: Option<&mut Option<String>>,
    mut open_claw_path: Option<&mut Option<String>>,
    config: &ConfigContext,
) {
    for options in config.option_maps() {
        let codex_options = CodexOptions::from_map(options);
        if let Some(speed) = codex_options.speed {
            *codex_speed = speed.into();
        }
        if let Some(pi_path) = pi_path.as_deref_mut()
            && let Some(path) = PiOptions::from_map(options).pi_path
        {
            *pi_path = Some(path);
        }
        if let Some(open_claw_path) = open_claw_path.as_deref_mut()
            && let Some(path) = OpenClawOptions::from_map(options).open_claw_path
        {
            *open_claw_path = Some(path);
        }
    }
}

impl crate::cli::CliConfig for ConfigContext {
    fn config_error(&self) -> Option<&str> {
        self.command_uses_named_pi_stores()
            .then_some(self.error.as_deref())
            .flatten()
    }

    fn apply_shared(&self, shared: &mut SharedArgs) {
        apply_config_to_shared(shared, self);
    }

    fn apply_daily_args(&self, args: &mut DailyArgs) {
        apply_config_to_daily_args(args, self);
    }

    fn apply_weekly_args(&self, args: &mut WeeklyArgs) {
        apply_config_to_weekly_args(args, self);
    }

    fn apply_blocks_args(&self, args: &mut BlocksArgs) {
        apply_config_to_blocks_args(args, self);
    }

    fn apply_statusline_args(&self, args: &mut StatuslineArgs) {
        apply_config_to_statusline_args(args, self);
    }

    fn apply_agent_args(
        &self,
        codex_speed: &mut CodexSpeed,
        pi_path: Option<&mut Option<String>>,
        open_claw_path: Option<&mut Option<String>>,
    ) {
        apply_config_to_agent_args(codex_speed, pi_path, open_claw_path, self);
    }
}

fn apply_shared_options(shared: &mut SharedArgs, options: SharedOptions) {
    if let Some(since) = options.since {
        shared.since = Some(normalize_date_bound(&since));
    }
    if let Some(until) = options.until {
        shared.until = Some(normalize_date_bound(&until));
    }
    if let Some(json) = options.json {
        shared.json = json;
    }
    if let Some(mode) = options.mode {
        shared.mode = mode.into();
    }
    if let Some(debug) = options.debug {
        shared.debug = debug;
    }
    if let Some(debug_samples) = options.debug_samples {
        shared.debug_samples = debug_samples;
    }
    if let Some(order) = options.order {
        shared.order = order.into();
    }
    if let Some(breakdown) = options.breakdown {
        shared.breakdown = breakdown;
    }
    if let Some(offline) = options.offline {
        shared.offline = offline;
    }
    if let Some(no_offline) = options.no_offline {
        shared.no_offline = no_offline;
    }
    if let Some(color) = options.color {
        shared.color = color;
    }
    if let Some(no_color) = options.no_color {
        shared.no_color = no_color;
    }
    if let Some(timezone) = options.timezone {
        shared.timezone = Some(timezone);
    }
    if let Some(jq) = options.jq {
        shared.jq = Some(jq);
    }
    if let Some(compact) = options.compact {
        shared.compact = compact;
    }
    if let Some(single_thread) = options.single_thread {
        shared.single_thread = single_thread;
    }
    if let Some(no_cost) = options.no_cost {
        shared.no_cost = no_cost;
    }
    if let Some(pricing_overrides) = options.pricing_overrides {
        merge_pricing_overrides(&mut shared.pricing_overrides, pricing_overrides);
    }
}

fn merge_pricing_overrides(
    current: &mut BTreeMap<String, PricingOverride>,
    incoming: BTreeMap<String, ConfigPricingOverride>,
) {
    for (model, incoming_override) in incoming {
        let entry = current.entry(model).or_default();
        merge_override_fields(entry, incoming_override);
    }
}

fn merge_override_fields(target: &mut PricingOverride, source: ConfigPricingOverride) {
    if source.input_cost_per_token.is_some() {
        target.input_cost_per_token = source.input_cost_per_token;
    }
    if source.output_cost_per_token.is_some() {
        target.output_cost_per_token = source.output_cost_per_token;
    }
    if source.cache_creation_input_token_cost.is_some() {
        target.cache_creation_input_token_cost = source.cache_creation_input_token_cost;
    }
    if source.cache_read_input_token_cost.is_some() {
        target.cache_read_input_token_cost = source.cache_read_input_token_cost;
    }
    if source.input_cost_per_token_above_200k_tokens.is_some() {
        target.input_cost_per_token_above_200k_tokens =
            source.input_cost_per_token_above_200k_tokens;
    }
    if source.output_cost_per_token_above_200k_tokens.is_some() {
        target.output_cost_per_token_above_200k_tokens =
            source.output_cost_per_token_above_200k_tokens;
    }
    if source
        .cache_creation_input_token_cost_above_200k_tokens
        .is_some()
    {
        target.cache_creation_input_token_cost_above_200k_tokens =
            source.cache_creation_input_token_cost_above_200k_tokens;
    }
    if source
        .cache_read_input_token_cost_above_200k_tokens
        .is_some()
    {
        target.cache_read_input_token_cost_above_200k_tokens =
            source.cache_read_input_token_cost_above_200k_tokens;
    }
    if source.max_input_tokens.is_some() {
        target.max_input_tokens = source.max_input_tokens;
    }
    if source.fast_multiplier.is_some() {
        target.fast_multiplier = source.fast_multiplier;
    }
}

impl From<ConfigPricingOverride> for PricingOverride {
    fn from(value: ConfigPricingOverride) -> Self {
        Self {
            input_cost_per_token: value.input_cost_per_token,
            output_cost_per_token: value.output_cost_per_token,
            cache_creation_input_token_cost: value.cache_creation_input_token_cost,
            cache_read_input_token_cost: value.cache_read_input_token_cost,
            input_cost_per_token_above_200k_tokens: value.input_cost_per_token_above_200k_tokens,
            output_cost_per_token_above_200k_tokens: value.output_cost_per_token_above_200k_tokens,
            cache_creation_input_token_cost_above_200k_tokens: value
                .cache_creation_input_token_cost_above_200k_tokens,
            cache_read_input_token_cost_above_200k_tokens: value
                .cache_read_input_token_cost_above_200k_tokens,
            max_input_tokens: value.max_input_tokens,
            fast_multiplier: value.fast_multiplier,
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

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::{
        DEFAULT_SESSION_DURATION_HOURS,
        cli::{
            BlocksArgs, CliConfig, CodexSpeed, CostMode, SortOrder, StatuslineArgs, VisualBurnRate,
            WeekDay, WeeklyArgs,
        },
    };
    use ccusage_test_support::fs_fixture;

    #[test]
    fn applies_schema_backed_shared_options() {
        let config = context(
            json!({
                "defaults": {
                    "since": "2026-01-01",
                    "until": "2026-01-31",
                    "json": true,
                    "mode": "calculate",
                    "debug": true,
                    "debugSamples": 9,
                    "order": "desc",
                    "breakdown": true,
                    "offline": true,
                    "noOffline": true,
                    "color": true,
                    "noColor": true,
                    "timezone": "Asia/Tokyo",
                    "jq": ".totals",
                    "compact": true,
                    "singleThread": true,
                    "noCost": true,
                }
            }),
            "daily",
            None,
            "daily",
        );
        let mut shared = SharedArgs::default();

        apply_config_to_shared(&mut shared, &config);

        assert_eq!(shared.since.as_deref(), Some("20260101"));
        assert_eq!(shared.until.as_deref(), Some("20260131"));
        assert!(shared.json);
        assert_eq!(shared.mode, CostMode::Calculate);
        assert!(shared.debug);
        assert_eq!(shared.debug_samples, 9);
        assert_eq!(shared.order, SortOrder::Desc);
        assert!(shared.breakdown);
        assert!(shared.offline);
        assert!(shared.no_offline);
        assert!(shared.color);
        assert!(shared.no_color);
        assert_eq!(shared.timezone.as_deref(), Some("Asia/Tokyo"));
        assert_eq!(shared.jq.as_deref(), Some(".totals"));
        assert!(shared.compact);
        assert!(shared.single_thread);
        assert!(shared.no_cost);
    }

    #[test]
    fn applies_schema_backed_report_specific_options() {
        let config = context(
            json!({
                "commands": {
                    "blocks": {
                        "active": true,
                        "recent": true,
                        "tokenLimit": "500000",
                        "sessionLength": 6.5
                    },
                    "statusline": {
                        "offline": false,
                        "noOffline": true,
                        "visualBurnRate": "emoji-text",
                        "costSource": "both",
                        "cache": false,
                        "noCache": true,
                        "refreshInterval": 3,
                        "contextLowThreshold": 45,
                        "contextMediumThreshold": 75,
                        "timezone": "Asia/Tokyo",
                        "debug": true
                    }
                }
            }),
            "blocks",
            None,
            "blocks",
        );
        let mut blocks = BlocksArgs {
            shared: SharedArgs::default(),
            active: false,
            recent: false,
            token_limit: None,
            session_length: DEFAULT_SESSION_DURATION_HOURS,
        };
        apply_config_to_blocks_args(&mut blocks, &config);

        assert!(blocks.active);
        assert!(blocks.recent);
        assert_eq!(blocks.token_limit.as_deref(), Some("500000"));
        assert_eq!(blocks.session_length, 6.5);

        let config = context(
            json!({
                "commands": {
                    "statusline": {
                        "offline": false,
                        "noOffline": true,
                        "visualBurnRate": "emoji-text",
                        "costSource": "both",
                        "cache": false,
                        "noCache": true,
                        "refreshInterval": 3,
                        "contextLowThreshold": 45,
                        "contextMediumThreshold": 75,
                        "timezone": "Asia/Tokyo",
                        "debug": true
                    }
                }
            }),
            "statusline",
            None,
            "statusline",
        );
        let mut statusline = StatuslineArgs::default();
        apply_config_to_statusline_args(&mut statusline, &config);

        assert!(!statusline.offline);
        assert!(statusline.no_offline);
        assert_eq!(statusline.visual_burn_rate, VisualBurnRate::EmojiText);
        assert_eq!(statusline.cost_source, crate::cli::CostSource::Both);
        assert!(!statusline.cache);
        assert!(statusline.no_cache);
        assert_eq!(statusline.refresh_interval, 3);
        assert_eq!(statusline.context_low_threshold, 45);
        assert_eq!(statusline.context_medium_threshold, 75);
        assert_eq!(statusline.timezone.as_deref(), Some("Asia/Tokyo"));
        assert!(statusline.debug);
    }

    #[test]
    fn applies_schema_backed_agent_specific_options() {
        let mut weekly = WeeklyArgs {
            shared: SharedArgs::default(),
            start_of_week: WeekDay::Sunday,
        };
        apply_config_to_weekly_args(
            &mut weekly,
            &context(
                json!({
                    "claude": {
                        "commands": {
                            "weekly": {
                                "startOfWeek": "monday"
                            }
                        }
                    }
                }),
                "claude weekly",
                Some("claude"),
                "weekly",
            ),
        );

        assert_eq!(weekly.start_of_week, WeekDay::Monday);

        let mut speed = CodexSpeed::Auto;
        apply_config_to_agent_args(
            &mut speed,
            None,
            None,
            &context(
                json!({
                    "codex": {
                        "defaults": {
                            "speed": "fast"
                        }
                    }
                }),
                "codex daily",
                Some("codex"),
                "daily",
            ),
        );

        assert_eq!(speed, CodexSpeed::Fast);

        let mut speed = CodexSpeed::Auto;
        let mut pi_path = None;
        apply_config_to_agent_args(
            &mut speed,
            Some(&mut pi_path),
            None,
            &context(
                json!({
                    "pi": {
                        "defaults": {
                            "piPath": "/tmp/pi-sessions"
                        }
                    }
                }),
                "pi daily",
                Some("pi"),
                "daily",
            ),
        );

        assert_eq!(pi_path.as_deref(), Some("/tmp/pi-sessions"));

        let mut speed = CodexSpeed::Auto;
        let mut open_claw_path = None;
        apply_config_to_agent_args(
            &mut speed,
            None,
            Some(&mut open_claw_path),
            &context(
                json!({
                    "openclaw": {
                        "defaults": {
                            "openClawPath": "/tmp/openclaw"
                        }
                    }
                }),
                "openclaw daily",
                Some("openclaw"),
                "daily",
            ),
        );

        assert_eq!(open_claw_path.as_deref(), Some("/tmp/openclaw"));
    }

    #[test]
    fn merge_pricing_overrides_field_level_preserves_parent_fields() {
        use crate::config_schema::ConfigPricingOverride;
        use ccusage_cli::PricingOverride;

        let mut current = BTreeMap::new();
        current.insert(
            "[pi] gpt-5.4".to_string(),
            PricingOverride {
                input_cost_per_token: Some(2.5e-6),
                output_cost_per_token: Some(1.5e-5),
                ..Default::default()
            },
        );

        // Child config only sets max_input_tokens for the same model
        let mut incoming = BTreeMap::new();
        incoming.insert(
            "[pi] gpt-5.4".to_string(),
            ConfigPricingOverride {
                max_input_tokens: Some(1_000_000),
                ..Default::default()
            },
        );

        merge_pricing_overrides(&mut current, incoming);

        let result = &current["[pi] gpt-5.4"];
        // Parent fields preserved
        assert_eq!(result.input_cost_per_token, Some(2.5e-6));
        assert_eq!(result.output_cost_per_token, Some(1.5e-5));
        // Child field applied
        assert_eq!(result.max_input_tokens, Some(1_000_000));
    }

    #[test]
    fn merge_pricing_overrides_child_overrides_parent_field() {
        use crate::config_schema::ConfigPricingOverride;
        use ccusage_cli::PricingOverride;

        let mut current = BTreeMap::new();
        current.insert(
            "model-a".to_string(),
            PricingOverride {
                input_cost_per_token: Some(3e-6),
                output_cost_per_token: Some(15e-6),
                cache_read_input_token_cost: Some(3e-7),
                ..Default::default()
            },
        );

        // Child overrides just input, leaves others alone
        let mut incoming = BTreeMap::new();
        incoming.insert(
            "model-a".to_string(),
            ConfigPricingOverride {
                input_cost_per_token: Some(2e-6),
                ..Default::default()
            },
        );

        merge_pricing_overrides(&mut current, incoming);

        let result = &current["model-a"];
        assert_eq!(result.input_cost_per_token, Some(2e-6)); // overridden
        assert_eq!(result.output_cost_per_token, Some(15e-6)); // preserved
        assert_eq!(result.cache_read_input_token_cost, Some(3e-7)); // preserved
    }

    #[test]
    fn accepts_named_pi_stores_from_config() {
        let config = config_context_from_json(
            r#"{
                "pi": {
                    "stores": [
                        { "name": "omp", "path": "~/.omp/agent/sessions" }
                    ]
                }
            }"#,
        );
        let mut shared = SharedArgs::default();

        assert_eq!(config.config_error(), None);
        apply_config_to_shared(&mut shared, &config);

        assert_eq!(shared.pi_stores.len(), 1);
        assert_eq!(shared.pi_stores[0].name, "omp");
        assert_eq!(shared.pi_stores[0].path, "~/.omp/agent/sessions");
    }

    #[test]
    fn treats_empty_named_pi_store_array_as_no_op() {
        let config = config_context_from_json(r#"{ "pi": { "stores": [] } }"#);
        let mut shared = SharedArgs::default();

        assert_eq!(config.config_error(), None);
        apply_config_to_shared(&mut shared, &config);

        assert!(shared.pi_stores.is_empty());
    }

    #[test]
    fn rejects_named_pi_store_bad_shape() {
        let config = config_context_from_json(r#"{ "pi": { "stores": [{ "name": "omp" }] } }"#);

        assert_eq!(
            config.config_error(),
            Some(
                "Invalid ccusage config: pi.stores[0] must contain string fields 'name' and 'path'"
            )
        );
    }

    #[test]
    fn rejects_named_pi_store_invalid_name() {
        let config = config_context_from_json(
            r#"{ "pi": { "stores": [{ "name": "Omp", "path": "/tmp/omp" }] } }"#,
        );

        assert_eq!(
            config.config_error(),
            Some("Invalid ccusage config: pi.stores[0].name must match ^[a-z][a-z0-9_-]{0,31}$")
        );
    }

    #[test]
    fn rejects_named_pi_store_builtin_agent_collision() {
        let config = config_context_from_json(
            r#"{ "pi": { "stores": [{ "name": "pi", "path": "/tmp/omp" }] } }"#,
        );

        assert_eq!(
            config.config_error(),
            Some("Invalid ccusage config: pi.stores name 'pi' collides with a built-in agent")
        );
    }

    #[test]
    fn rejects_named_pi_store_duplicate_name() {
        let config = config_context_from_json(
            r#"{
                "pi": {
                    "stores": [
                        { "name": "omp", "path": "/tmp/omp-a" },
                        { "name": "omp", "path": "/tmp/omp-b" }
                    ]
                }
            }"#,
        );

        assert_eq!(
            config.config_error(),
            Some("Invalid ccusage config: duplicate pi.stores name 'omp'")
        );
    }

    #[test]
    fn rejects_named_pi_store_empty_path_with_store_name() {
        let config = config_context_from_json(
            r#"{ "pi": { "stores": [{ "name": "omp", "path": "  " }] } }"#,
        );

        assert_eq!(
            config.config_error(),
            Some("Invalid ccusage config: pi.stores[0] ('omp'): path must be a non-empty string")
        );
    }

    #[test]
    fn named_pi_store_validation_does_not_break_statusline() {
        let fixture = fs_fixture!({
            "ccusage.json": r#"{ "pi": { "stores": [{ "name": "codex", "path": "/tmp/omp" }] } }"#,
        });
        let args = vec![
            "statusline".to_string(),
            "--config".to_string(),
            fixture.path("ccusage.json").to_string_lossy().into_owned(),
        ];
        let config = ConfigContext::from_args(&args);

        let parsed = crate::cli::Cli::parse_from_with_config(
            std::iter::once(std::ffi::OsString::from("ccusage")).chain(
                args.iter()
                    .map(|arg| std::ffi::OsString::from(arg.as_str())),
            ),
            &config,
            crate::DEFAULT_SESSION_DURATION_HOURS,
            env!("CARGO_PKG_VERSION"),
        );

        assert!(parsed.is_ok());
    }

    #[test]
    fn named_pi_store_validation_does_not_break_agent_commands() {
        let fixture = fs_fixture!({
            "ccusage.json": r#"{ "pi": { "stores": [{ "name": "codex", "path": "/tmp/omp" }] } }"#,
        });
        let args = vec![
            "codex".to_string(),
            "daily".to_string(),
            "--config".to_string(),
            fixture.path("ccusage.json").to_string_lossy().into_owned(),
        ];
        let config = ConfigContext::from_args(&args);

        let parsed = crate::cli::Cli::parse_from_with_config(
            std::iter::once(std::ffi::OsString::from("ccusage")).chain(
                args.iter()
                    .map(|arg| std::ffi::OsString::from(arg.as_str())),
            ),
            &config,
            crate::DEFAULT_SESSION_DURATION_HOURS,
            env!("CARGO_PKG_VERSION"),
        );

        assert!(parsed.is_ok());
    }

    #[test]
    fn reserved_named_pi_store_names_match_unified_loader_agents() {
        let reserved = reserved_named_pi_store_names()
            .into_iter()
            .collect::<BTreeSet<_>>();
        let expected = std::iter::once("all")
            .chain(crate::adapter::all::BUILT_IN_AGENT_NAMES.iter().copied())
            .collect::<BTreeSet<_>>();

        assert_eq!(reserved, expected);
    }

    #[test]
    fn named_pi_store_validation_uses_schema_pattern() {
        assert_eq!(
            NAMED_PI_STORE_NAME_PATTERN,
            crate::config_schema::NAMED_PI_STORE_NAME_PATTERN
        );
        assert!(validate_named_pi_store_name("omp").is_ok());
        assert!(validate_named_pi_store_name("o3-fork").is_ok());
        assert!(validate_named_pi_store_name("Omp").is_err());
        assert!(validate_named_pi_store_name("all").is_err());
        assert!(validate_named_pi_store_name("codex").is_err());
    }

    #[test]
    fn reports_named_pi_store_validation_through_cli_config_error_path() {
        let fixture = fs_fixture!({
            "ccusage.json": r#"{ "pi": { "stores": [{ "name": "codex", "path": "/tmp/omp" }] } }"#,
        });
        let args = vec![
            "daily".to_string(),
            "--config".to_string(),
            fixture.path("ccusage.json").to_string_lossy().into_owned(),
        ];
        let config = ConfigContext::from_args(&args);

        let result = crate::cli::Cli::parse_from_with_config(
            std::iter::once(std::ffi::OsString::from("ccusage")).chain(
                args.iter()
                    .map(|arg| std::ffi::OsString::from(arg.as_str())),
            ),
            &config,
            crate::DEFAULT_SESSION_DURATION_HOURS,
            env!("CARGO_PKG_VERSION"),
        );
        let Err(error) = result else {
            panic!("expected config error");
        };

        assert_eq!(
            error,
            "Invalid ccusage config: pi.stores name 'codex' collides with a built-in agent"
        );
    }

    fn context(value: Value, raw: &str, agent: Option<&str>, report: &str) -> ConfigContext {
        ConfigContext {
            value: Some(value),
            command: ConfigCommand {
                raw: raw.to_string(),
                agent: agent.map(ToString::to_string),
                report: report.to_string(),
            },
            pi_stores: Vec::new(),
            error: None,
        }
    }

    fn config_context_from_json(raw: &str) -> ConfigContext {
        let fixture = fs_fixture!({
            "ccusage.json": raw,
        });
        ConfigContext::from_args(&[
            "daily".to_string(),
            "--config".to_string(),
            fixture.path("ccusage.json").to_string_lossy().into_owned(),
        ])
    }
}
