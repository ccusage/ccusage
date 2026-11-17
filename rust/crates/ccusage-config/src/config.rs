use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

use ccusage_cli::{
    AgentOptions, BlocksArgs, CodexSpeed, ColorChoice, CostMode, CostSource, DailyArgs,
    NamedPiStore, PricingOverride, SharedArgs, SortOrder, StatuslineArgs, VisualBurnRate, WeekDay,
    WeeklyArgs,
};

use crate::config_schema::{
    ConfigCodexSpeed, ConfigCostMode, ConfigCostSource, ConfigPricingOverride, ConfigSortOrder,
    ConfigVisualBurnRate, ConfigWeekDay, NAMED_PI_STORE_NAME_PATTERN, PiStoreConfig, SharedOptions,
};

struct ConfigCommand {
    raw: String,
    agent: Option<String>,
    report: String,
}

pub struct ConfigContext {
    command: ConfigCommand,
    pi_stores: Vec<NamedPiStore>,
    error: Option<String>,
    layers: BTreeMap<String, crate::layers::Layer>,
    warnings: Vec<String>,
}

impl ConfigContext {
    pub fn from_args(args: &[String]) -> Self {
        let command = detect_config_command(args);
        if args
            .iter()
            .any(|arg| matches!(arg.as_str(), "-h" | "--help" | "-v" | "-V" | "--version"))
        {
            return Self::empty(command);
        }
        let (file, error, warnings) = load_config_value(scan_config_path(args).as_deref());
        let mut context = Self::empty(command);
        context.error = error;
        context.warnings = warnings;
        if let Some(file) = file {
            context.pi_stores = file.pi_stores;
            context.layers = file.layers;
        }
        context
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    fn empty(command: ConfigCommand) -> Self {
        Self {
            command,
            pi_stores: Vec::new(),
            error: None,
            layers: BTreeMap::new(),
            warnings: Vec::new(),
        }
    }

    pub fn for_dependency(&self, agent: &str) -> Self {
        let report = &self.command.report;
        Self {
            command: ConfigCommand {
                raw: format!("{agent} {report}"),
                agent: Some(agent.to_string()),
                report: report.clone(),
            },
            pi_stores: self.pi_stores.clone(),
            error: self.error.clone(),
            layers: self.layers.clone(),
            warnings: self.warnings.clone(),
        }
    }

    fn option_maps(&self) -> Vec<&crate::layers::Layer> {
        let report = &self.command.report;
        let mut paths = vec!["defaults".to_string()];
        if let Some(agent) = &self.command.agent {
            paths.extend([
                format!("commands.{report}"),
                format!("commands.{}", self.command.raw),
                format!("commands.{agent}:{report}"),
                format!("{agent}.defaults"),
                format!("{agent}.commands.{report}"),
            ]);
        } else {
            paths.push(format!("commands.{}", self.command.raw));
        }
        paths
            .iter()
            .filter_map(|path| self.layers.get(path))
            .collect()
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
    let stores = crate::layers::decode::<Vec<PiStoreConfig>>(stores_value, "pi.stores")
        .map_err(config_error)?;

    let mut names = BTreeSet::new();
    let mut parsed = Vec::with_capacity(stores.len());
    for (index, store) in stores.iter().enumerate() {
        let name = &store.name;
        let path = &store.path;
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
        .chain(ccusage_cli::BUILT_IN_AGENT_NAMES.iter().copied())
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

fn load_config_value(
    path: Option<&Path>,
) -> (
    Option<crate::layers::ConfigFile>,
    Option<String>,
    Vec<String>,
) {
    load_config_paths(
        path.is_some(),
        match path {
            Some(path) => vec![path.to_path_buf()],
            None => discover_config_paths(),
        },
    )
}

fn load_config_paths(
    explicit: bool,
    paths: Vec<PathBuf>,
) -> (
    Option<crate::layers::ConfigFile>,
    Option<String>,
    Vec<String>,
) {
    let mut warnings = Vec::new();
    for path in paths {
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if !explicit && error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                let message = config_error(format!("{}: {error}", path.display()));
                if explicit {
                    return (None, Some(message), warnings);
                }
                warnings.push(message);
                continue;
            }
        };
        let file = serde_json::from_str(&content)
            .map_err(|error| error.to_string())
            .and_then(|value| {
                if explicit {
                    crate::layers::ConfigFile::parse(value)
                } else {
                    crate::layers::ConfigFile::parse_discovered(value)
                }
            });
        match file {
            Ok(mut file) => {
                match parse_named_pi_stores(&file.value) {
                    Ok(stores) => file.pi_stores = stores,
                    Err(error) => {
                        let detail = error
                            .strip_prefix("Invalid ccusage config: ")
                            .unwrap_or(&error);
                        let message = config_error(format!("{}: {detail}", path.display()));
                        return (None, Some(message), warnings);
                    }
                }
                warnings.extend(
                    file.warnings
                        .drain(..)
                        .map(|warning| format!("{}: {warning}", path.display())),
                );
                return (Some(file), None, warnings);
            }
            Err(error) => {
                let message = config_error(format!("{}: {error}", path.display()));
                return (None, Some(message), warnings);
            }
        }
    }
    (None, None, warnings)
}

fn discover_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(cwd) = env::current_dir() {
        paths.push(cwd.join(".ccusage").join("ccusage.json"));
    }
    paths.extend(
        ccusage_cli::home::claude_config_dirs()
            .into_dirs()
            .into_iter()
            .map(|dir| dir.join("ccusage.json")),
    );
    paths
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
    if let Some((agent, report)) = ccusage_cli::AgentId::from_colon_command(first) {
        return ConfigCommand {
            raw: format!("{} {report}", agent.spec().name),
            agent: Some(agent.spec().name.to_string()),
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
            | "--last"
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
            | "--sections"
    )
}

fn is_agent_command(command: &str) -> bool {
    ccusage_cli::BUILT_IN_AGENT_NAMES.contains(&command)
}

fn is_report_command(command: &str) -> bool {
    matches!(
        command,
        "daily" | "monthly" | "weekly" | "session" | "blocks" | "statusline"
    )
}

fn apply_config_to_shared(shared: &mut SharedArgs, config: &ConfigContext) {
    for options in config.option_maps() {
        apply_shared_options(shared, options.shared.clone());
    }
    shared.pi_stores = config.pi_stores.clone();
}

fn apply_config_to_daily_args(args: &mut DailyArgs, config: &ConfigContext) {
    for options in config.option_maps() {
        let options = options.daily.clone();
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

fn apply_config_to_weekly_args(args: &mut WeeklyArgs, config: &ConfigContext) {
    for options in config.option_maps() {
        if let Some(day) = options.weekly.clone().start_of_week {
            args.start_of_week = day.into();
        }
    }
}

fn apply_config_to_blocks_args(args: &mut BlocksArgs, config: &ConfigContext) {
    for options in config.option_maps() {
        let options = options.blocks.clone();
        if let Some(active) = options.active {
            args.active = active;
        }
        if let Some(recent) = options.recent {
            args.recent = recent;
        }
        if let Some(token_limit) = options.token_limit {
            args.token_limit = Some(token_limit.into());
        }
        if let Some(session_length) = options.session_length {
            args.session_length = session_length;
        }
    }
}

fn apply_config_to_statusline_args(args: &mut StatuslineArgs, config: &ConfigContext) {
    for options in config.option_maps() {
        let options = options.statusline.clone();
        if let Some(offline) = options.offline {
            args.offline = offline;
        }
        if options.no_offline == Some(true) {
            args.offline = false;
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
        if options.no_cache == Some(true) {
            args.cache = false;
        }
        if let Some(refresh_interval) = options.refresh_interval {
            args.refresh_interval = refresh_interval;
        }
        if let Some(threshold) = options.context_low_threshold {
            args.context_low_threshold = threshold;
        }
        if let Some(threshold) = options.context_medium_threshold {
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
        if let Some(pricing_overrides) = options.pricing_overrides {
            merge_pricing_overrides(&mut args.pricing_overrides, pricing_overrides);
        }
    }
}

impl ccusage_cli::CliConfig for ConfigContext {
    fn agent_options(&self, agent: &str) -> AgentOptions {
        let mut resolved = AgentOptions::default();
        for options in self.option_maps() {
            match agent {
                "claude" if self.command.report == "daily" => {
                    if let Some(project) = options.daily.clone().project {
                        resolved.claude_project = Some(project);
                    }
                }
                "codex" => {
                    if let Some(speed) = options.codex.clone().speed {
                        resolved.codex_speed = speed.into();
                    }
                }
                "pi" => {
                    if let Some(path) = options.pi.clone().pi_path {
                        resolved.pi_path = Some(path);
                    }
                }
                "openclaw" => {
                    if let Some(path) = options.openclaw.clone().open_claw_path {
                        resolved.open_claw_path = Some(path);
                    }
                }
                _ => {}
            }
        }
        resolved
    }

    fn config_error(&self) -> Option<&str> {
        self.error.as_deref()
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
}

fn apply_shared_options(shared: &mut SharedArgs, options: SharedOptions) {
    if let Some(since) = options.since {
        shared.since = Some(since.0);
    }
    if let Some(until) = options.until {
        shared.until = Some(until.0);
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
        shared.order = Some(order.into());
    }
    if let Some(breakdown) = options.breakdown {
        shared.breakdown = breakdown;
    }
    if let Some(offline) = options.offline {
        shared.offline = offline;
    }
    if options.no_offline == Some(true) {
        shared.offline = false;
    }
    if options.color == Some(true) {
        shared.color = ColorChoice::Always;
    }
    if options.no_color == Some(true) {
        shared.color = ColorChoice::Never;
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
    if source.cache_creation_input_token_cost_above_1hr.is_some() {
        target.cache_creation_input_token_cost_above_1hr =
            source.cache_creation_input_token_cost_above_1hr;
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
    if source.flex_multiplier.is_some() {
        target.flex_multiplier = source.flex_multiplier;
    }
}

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

#[cfg(test)]
mod tests {
    #[test]
    fn token_limits_accept_numbers_and_strings() {
        for raw in [
            r#"{"commands":{"blocks":{"tokenLimit":500000}}}"#,
            r#"{"commands":{"blocks":{"tokenLimit":"500000"}}}"#,
        ] {
            let fixture = fs_fixture!({"config.json": raw});
            let config = ConfigContext::from_args(&[
                "blocks".into(),
                "--config".into(),
                fixture.path("config.json").display().to_string(),
            ]);
            assert!(
                config.config_error().is_none(),
                "{:?}",
                config.config_error()
            );
            let mut args = BlocksArgs {
                shared: SharedArgs::default(),
                active: false,
                recent: false,
                token_limit: None,
                session_length: 5.0,
            };
            config.apply_blocks_args(&mut args);
            assert_eq!(args.token_limit.as_deref(), Some("500000"));
        }
    }
    #[test]
    fn discovered_config_retains_valid_fields_without_falling_through() {
        let fixture = fs_fixture!({
            "selected.json": r#"{"defaults":{"json":true,"timezone":"Asia/Tokyo","mode":"Calculate","pricingOverrides":{"sample":{"inputCostPerToken":0.1,"outputCostPerToken":"bad"}}},"commands":{"weekly":{"startOfWeek":"Sunday"}}}"#,
            "lower.json": r#"{"defaults":{"json":false,"timezone":"UTC"}}"#,
        });
        let (file, error, warnings) = load_config_paths(
            false,
            vec![fixture.path("selected.json"), fixture.path("lower.json")],
        );
        assert!(error.is_none(), "{error:?}");
        let file = file.unwrap();
        assert_eq!(file.layers["defaults"].shared.json, Some(true));
        assert_eq!(
            file.layers["defaults"].shared.timezone.as_deref(),
            Some("Asia/Tokyo")
        );
        assert_eq!(
            file.layers["defaults"]
                .shared
                .pricing_overrides
                .as_ref()
                .unwrap()["sample"]
                .input_cost_per_token,
            Some(0.1)
        );
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        for path in [
            "defaults.mode",
            "defaults.pricingOverrides.sample.outputCostPerToken",
            "commands.weekly.startOfWeek",
        ] {
            assert!(
                warnings.iter().any(|warning| warning.contains(path)),
                "{warnings:?}"
            );
        }
    }

    #[test]
    fn discovered_invalid_bounds_and_pi_stores_stay_fatal() {
        for raw in [
            r#"{"defaults":{"since":"2026-13-45"}}"#,
            r#"{"defaults":{"until":"no"}}"#,
            r#"{"pi":{"stores":[{"name":"claude","path":"/tmp/store"}]}}"#,
            r#"{"pi":{"stores":[{"name":"custom"}]}}"#,
        ] {
            let fixture = fs_fixture!({"selected.json": raw});
            let (file, error, _) = load_config_paths(false, vec![fixture.path("selected.json")]);
            assert!(file.is_none());
            assert!(error.is_some(), "{raw}");
        }
    }

    #[test]
    fn unknown_keys_warn_without_discarding_valid_options() {
        let config = config_context_from_json(
            r#"{"defaults":{"json":true,"jsoon":true,"pricingOverrides":{"model":{"inputCostPerToken":0.1,"inputPrice":9}}},"commands":{"blocks":{"live":true}},"cluade":{}}"#,
        );
        assert!(config.config_error().is_none());
        let mut shared = SharedArgs::default();
        config.apply_shared(&mut shared);
        assert!(shared.json);
        assert_eq!(
            shared.pricing_overrides["model"].input_cost_per_token,
            Some(0.1)
        );
        for path in [
            "defaults.jsoon",
            "defaults.pricingOverrides.model.inputPrice",
            "commands.blocks.live",
            "cluade",
        ] {
            assert!(
                config
                    .warnings
                    .iter()
                    .any(|warning| warning.ends_with(path)),
                "{path}: {:?}",
                config.warnings
            );
        }
        assert_eq!(config.warnings.len(), 4);
    }

    #[test]
    fn explicit_config_rejects_wrong_types_and_bad_enums_with_paths() {
        for (raw, path) in [
            (r#"{"defaults":{"json":"yes"}}"#, "defaults.json"),
            (
                r#"{"commands":{"daily":{"mode":"invalid"}}}"#,
                "commands.daily.mode",
            ),
            (
                r#"{"commands":{"statusline":{"contextLowThreshold":256}}}"#,
                "commands.statusline.contextLowThreshold",
            ),
            (
                r#"{"defaults":{"pricingOverrides":{"model":{"inputCostPerToken":"cheap"}}}}"#,
                "defaults.pricingOverrides.model.inputCostPerToken",
            ),
        ] {
            let config = config_context_from_json(raw);
            let error = config
                .config_error()
                .expect("invalid explicit file must fail");
            assert!(error.contains(path), "{error}");
        }
    }

    #[test]
    fn explicit_config_rejects_broken_and_missing_files() {
        let fixture = fs_fixture!({ "broken.json": "{" });
        for name in ["broken.json", "missing.json"] {
            let path = fixture.path(name);
            let config = ConfigContext::from_args(&[
                "daily".into(),
                "--config".into(),
                path.display().to_string(),
            ]);
            let error = config
                .config_error()
                .expect("invalid explicit path must fail");
            assert!(error.contains(name), "{error}");
        }
    }

    #[test]
    fn colon_commands_select_only_registered_reports() {
        for (name, agent) in [
            ("codex:daily", Some("codex")),
            ("claude:statusline", Some("claude")),
            ("codex:weekly", None),
            ("unknown:daily", None),
        ] {
            let command = super::detect_config_command(&[name.to_string()]);
            assert_eq!(command.agent.as_deref(), agent, "{name}");
        }
    }

    use serde_json::{Value, json};

    use super::*;
    use ccusage_cli::DEFAULT_SESSION_DURATION_HOURS;
    use ccusage_cli::{
        BlocksArgs, CliConfig, CodexSpeed, CostMode, SortOrder, StatuslineArgs, VisualBurnRate,
        WeekDay, WeeklyArgs,
    };
    use ccusage_test_support::{EnvVarsGuard, fs_fixture};

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
        assert_eq!(shared.order, Some(SortOrder::Desc));
        assert!(shared.breakdown);
        assert!(!shared.offline);
        assert_eq!(shared.color, ColorChoice::Never);
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
        assert_eq!(statusline.visual_burn_rate, VisualBurnRate::EmojiText);
        assert_eq!(statusline.cost_source, ccusage_cli::CostSource::Both);
        assert!(!statusline.cache);
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

        let codex = context(
            json!({"codex":{"defaults":{"speed":"fast"}}}),
            "codex daily",
            Some("codex"),
            "daily",
        );
        assert_eq!(codex.agent_options("codex").codex_speed, CodexSpeed::Fast);
        let pi = context(
            json!({"pi":{"defaults":{"piPath":"/tmp/pi-sessions"}}}),
            "pi daily",
            Some("pi"),
            "daily",
        );
        assert_eq!(
            pi.agent_options("pi").pi_path.as_deref(),
            Some("/tmp/pi-sessions")
        );
        let openclaw = context(
            json!({"openclaw":{"defaults":{"openClawPath":"/tmp/openclaw"}}}),
            "openclaw daily",
            Some("openclaw"),
            "daily",
        );
        assert_eq!(
            openclaw.agent_options("openclaw").open_claw_path.as_deref(),
            Some("/tmp/openclaw")
        );
    }

    #[test]
    fn grok_namespace_keeps_shared_report_options() {
        let config = context(
            json!({
                "grok": {
                    "defaults": { "offline": true },
                    "commands": { "session": { "json": true } }
                }
            }),
            "grok session",
            Some("grok"),
            "session",
        );
        let mut shared = SharedArgs::default();

        apply_config_to_shared(&mut shared, &config);

        assert!(shared.offline);
        assert!(shared.json);
    }

    #[test]
    fn zcode_namespace_keeps_shared_report_options() {
        let config = context(
            json!({
                "zcode": {
                    "defaults": { "offline": true },
                    "commands": { "session": { "json": true } }
                }
            }),
            "zcode session",
            Some("zcode"),
            "session",
        );
        let mut shared = SharedArgs::default();

        apply_config_to_shared(&mut shared, &config);

        assert!(shared.offline);
        assert!(shared.json);
    }

    #[test]
    fn agent_command_sections_override_the_generic_report_section() {
        let generic = json!({ "mode": "display", "since": "20260101" });
        let cases = [
            (
                "space form",
                json!({ "daily": generic, "codex daily": { "mode": "calculate" } }),
                CostMode::Calculate,
            ),
            (
                "colon form",
                json!({ "daily": generic, "codex:daily": { "mode": "calculate" } }),
                CostMode::Calculate,
            ),
            (
                "colon form over space form",
                json!({
                    "daily": generic,
                    "codex daily": { "mode": "calculate" },
                    "codex:daily": { "mode": "auto" }
                }),
                CostMode::Auto,
            ),
        ];
        for (name, commands, mode) in cases {
            let value = json!({ "commands": commands });
            let focused = context(value.clone(), "codex daily", Some("codex"), "daily");
            let source = context(value, "daily", None, "daily").for_dependency("codex");

            for config in [focused, source] {
                let mut shared = SharedArgs::default();
                config.apply_shared(&mut shared);
                assert_eq!(shared.mode, mode, "{name}");
                assert_eq!(shared.since.as_deref(), Some("20260101"), "{name}");
            }
        }
    }

    #[test]
    fn space_form_section_overrides_the_generic_report_section_for_every_agent_command_shape() {
        let raw = r#"{
            "commands": {
                "daily": { "mode": "display", "since": "20260101" },
                "codex daily": { "mode": "calculate" }
            }
        }"#;
        for command in [&["codex", "daily"][..], &["codex:daily"], &["codex"]] {
            let config = config_context_for_command(raw, command);
            let mut shared = SharedArgs::default();
            config.apply_shared(&mut shared);
            assert_eq!(shared.mode, CostMode::Calculate, "{command:?}");
            assert_eq!(shared.since.as_deref(), Some("20260101"), "{command:?}");
        }
    }

    #[test]
    fn source_view_reads_the_sections_a_focused_command_would() {
        let config = context(
            json!({
                "defaults": {
                    "mode": "display",
                    "since": "20260101"
                },
                "commands": {
                    "daily": {
                        "breakdown": true
                    },
                    "codex:daily": {
                        "timezone": "UTC"
                    }
                },
                "codex": {
                    "defaults": {
                        "speed": "fast",
                        "since": "20260301"
                    },
                    "commands": {
                        "daily": {
                            "mode": "calculate"
                        }
                    }
                }
            }),
            "daily",
            None,
            "daily",
        );

        let codex = config.for_dependency("codex");
        let mut shared = SharedArgs::with_defaults();
        codex.apply_shared(&mut shared);
        let speed = codex.agent_options("codex").codex_speed;

        assert_eq!(shared.since.as_deref(), Some("20260301"));
        assert_eq!(shared.mode, CostMode::Calculate);
        assert_eq!(shared.timezone.as_deref(), Some("UTC"));
        assert!(shared.breakdown);
        assert_eq!(speed, CodexSpeed::Fast);
    }

    #[test]
    fn cline_weekly_config_matches_focused_and_unified_source_views() {
        let value = json!({
            "defaults": {
                "mode": "auto",
                "since": "20260101",
                "pricingOverrides": {
                    "cline-model": {
                        "inputCostPerToken": 0.000001
                    }
                }
            },
            "commands": {
                "weekly": {
                    "mode": "display"
                },
                "cline:weekly": {
                    "pricingOverrides": {
                        "cline-model": {
                            "inputCostPerToken": 0.000002
                        }
                    }
                }
            },
            "cline": {
                "defaults": {
                    "mode": "display",
                    "offline": true,
                    "since": "20260301",
                    "pricingOverrides": {
                        "cline-model": {
                            "outputCostPerToken": 0.00002
                        }
                    }
                },
                "commands": {
                    "weekly": {
                        "mode": "calculate",
                        "json": true,
                        "timezone": "UTC",
                        "pricingOverrides": {
                            "cline-model": {
                                "inputCostPerToken": 0.000003
                            }
                        }
                    }
                }
            }
        });
        let unified = context(value.clone(), "weekly", None, "weekly");
        let focused = context(value, "cline weekly", Some("cline"), "weekly");
        let unified_source = unified.for_dependency("cline");

        for config in [&focused, &unified_source] {
            let mut shared = SharedArgs::default();
            apply_config_to_shared(&mut shared, config);

            assert_eq!(shared.mode, CostMode::Calculate);
            assert_eq!(shared.since.as_deref(), Some("20260301"));
            assert!(shared.offline);
            assert!(shared.json);
            assert_eq!(shared.timezone.as_deref(), Some("UTC"));
            assert_eq!(
                shared.pricing_overrides["cline-model"].input_cost_per_token,
                Some(0.000003)
            );
            assert_eq!(
                shared.pricing_overrides["cline-model"].output_cost_per_token,
                Some(0.00002)
            );
        }
    }

    #[test]
    fn dependency_views_resolve_effective_options_without_raw_json() {
        let config = context(
            json!({
                "defaults": {
                    "mode": "display"
                },
                "commands": {
                    "pi:daily": {
                        "json": true
                    }
                },
                "openclaw": {
                    "defaults": {
                        "offline": true
                    }
                },
                "cline": {
                    "commands": {
                        "weekly": {
                            "json": true
                        }
                    }
                }
            }),
            "daily",
            None,
            "daily",
        );

        let mut pi = SharedArgs::default();
        config.for_dependency("pi").apply_shared(&mut pi);
        assert!(pi.json);
        let mut openclaw = SharedArgs::default();
        config
            .for_dependency("openclaw")
            .apply_shared(&mut openclaw);
        assert!(openclaw.offline);
        let mut codex = SharedArgs::default();
        config.for_dependency("codex").apply_shared(&mut codex);
        assert_eq!(codex.mode, CostMode::Display);

        let focused = context(
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
        );

        assert_eq!(
            focused
                .for_dependency("codex")
                .agent_options("codex")
                .codex_speed,
            CodexSpeed::Fast
        );
    }

    #[test]
    fn source_view_rejects_invalid_source_date_bounds() {
        let config = context(
            json!({"codex":{"defaults":{"since":"2026-02-30"}}}),
            "daily",
            None,
            "daily",
        );
        let source = config.for_dependency("codex");
        assert_eq!(
            source.config_error(),
            Some(
                "Invalid ccusage config: codex.defaults.since: not a valid date. Expected YYYY-MM-DD or YYYYMMDD"
            )
        );
    }

    #[test]
    fn one_hour_cache_rates_survive_parsing_and_partial_config_merges() {
        let parent: ConfigPricingOverride = serde_json::from_str(
            r#"{"inputCostPerToken":0.000001,"cacheCreationInputTokenCostAbove1hr":0.000007}"#,
        )
        .unwrap();
        let mut current = BTreeMap::from([("model".to_string(), parent.into())]);
        let child: ConfigPricingOverride =
            serde_json::from_str(r#"{"outputCostPerToken":0.000002}"#).unwrap();
        merge_pricing_overrides(&mut current, BTreeMap::from([("model".to_string(), child)]));
        assert_eq!(
            current["model"].cache_creation_input_token_cost_above_1hr,
            Some(7e-6)
        );
        let child: ConfigPricingOverride =
            serde_json::from_str(r#"{"cacheCreationInputTokenCostAbove1hr":0}"#).unwrap();
        merge_pricing_overrides(&mut current, BTreeMap::from([("model".to_string(), child)]));
        assert_eq!(
            current["model"].cache_creation_input_token_cost_above_1hr,
            Some(0.0)
        );
        assert_eq!(current["model"].input_cost_per_token, Some(1e-6));
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
                fast_multiplier: Some(2.0),
                flex_multiplier: Some(0.75),
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
        assert_eq!(result.fast_multiplier, Some(2.0));
        assert_eq!(result.flex_multiplier, Some(0.75));
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
                fast_multiplier: Some(2.0),
                flex_multiplier: Some(0.75),
                ..Default::default()
            },
        );

        // Child overrides just input, leaves others alone
        let mut incoming = BTreeMap::new();
        incoming.insert(
            "model-a".to_string(),
            ConfigPricingOverride {
                input_cost_per_token: Some(2e-6),
                fast_multiplier: Some(3.0),
                flex_multiplier: Some(0.5),
                ..Default::default()
            },
        );

        merge_pricing_overrides(&mut current, incoming);

        let result = &current["model-a"];
        assert_eq!(result.input_cost_per_token, Some(2e-6)); // overridden
        assert_eq!(result.output_cost_per_token, Some(15e-6)); // preserved
        assert_eq!(result.cache_read_input_token_cost, Some(3e-7)); // preserved
        assert_eq!(result.fast_multiplier, Some(3.0));
        assert_eq!(result.flex_multiplier, Some(0.5));
    }

    #[test]
    fn merge_pricing_overrides_preserves_flex_multiplier_for_new_models() {
        let mut current = BTreeMap::new();
        let incoming = BTreeMap::from([(
            "new-model".to_string(),
            ConfigPricingOverride {
                input_cost_per_token: Some(1e-6),
                flex_multiplier: Some(0.5),
                ..Default::default()
            },
        )]);

        merge_pricing_overrides(&mut current, incoming);

        assert_eq!(current["new-model"].input_cost_per_token, Some(1e-6));
        assert_eq!(current["new-model"].flex_multiplier, Some(0.5));
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

        assert_config_error(&config, "pi.stores[0]: missing field `path`");
    }

    #[test]
    fn rejects_named_pi_store_invalid_name() {
        let config = config_context_from_json(
            r#"{ "pi": { "stores": [{ "name": "Omp", "path": "/tmp/omp" }] } }"#,
        );

        assert_config_error(
            &config,
            "pi.stores[0].name must match ^[a-z][a-z0-9_-]{0,31}$",
        );
    }

    #[test]
    fn rejects_named_pi_store_builtin_agent_collision() {
        let config = config_context_from_json(
            r#"{ "pi": { "stores": [{ "name": "pi", "path": "/tmp/omp" }] } }"#,
        );

        assert_config_error(
            &config,
            "pi.stores name 'pi' collides with a built-in agent",
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

        assert_config_error(&config, "duplicate pi.stores name 'omp'");
    }

    #[test]
    fn rejects_named_pi_store_empty_path_with_store_name() {
        let config = config_context_from_json(
            r#"{ "pi": { "stores": [{ "name": "omp", "path": "  " }] } }"#,
        );

        assert_config_error(
            &config,
            "pi.stores[0] ('omp'): path must be a non-empty string",
        );
    }

    #[test]
    fn reserved_named_pi_store_names_match_unified_loader_agents() {
        let reserved = reserved_named_pi_store_names()
            .into_iter()
            .collect::<BTreeSet<_>>();
        let expected = std::iter::once("all")
            .chain(ccusage_cli::BUILT_IN_AGENT_NAMES.iter().copied())
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
    fn rejects_config_date_bounds_that_are_not_real_calendar_dates() {
        let config = config_context_from_json(r#"{ "defaults": { "since": "2026-02-30" } }"#);

        assert_config_error(
            &config,
            "defaults.since: not a valid date. Expected YYYY-MM-DD or YYYYMMDD",
        );

        let config = config_context_from_json(r#"{ "commands": { "daily": { "until": "abc" } } }"#);

        assert_config_error(
            &config,
            "commands.daily.until: not a valid date. Expected YYYY-MM-DD or YYYYMMDD",
        );
    }

    #[test]
    fn rejects_config_date_bounds_that_are_not_strings() {
        let config = config_context_from_json(r#"{ "defaults": { "since": 20260710 } }"#);

        assert_config_error(
            &config,
            "defaults.since: invalid type: integer `20260710`, expected a string",
        );
    }

    #[test]
    fn detects_commands_after_root_options_with_values() {
        for command in [
            vec!["--last", "1", "weekly"],
            vec!["--sections", "daily", "weekly"],
        ] {
            let config = config_context_for_command(
                r#"{ "commands": { "weekly": { "since": "not-a-date" } } }"#,
                &command,
            );

            assert_config_error(
                &config,
                "commands.weekly.since: not a valid date. Expected YYYY-MM-DD or YYYYMMDD",
            );
        }
    }

    #[test]
    fn rejects_config_date_bounds_for_commands_without_named_pi_stores() {
        for command in [
            vec!["blocks"],
            vec!["statusline"],
            vec!["all", "daily"],
            vec!["codex", "daily"],
        ] {
            let config = config_context_for_command(
                r#"{ "defaults": { "since": "2026-02-30" } }"#,
                &command,
            );

            assert_config_error(
                &config,
                "defaults.since: not a valid date. Expected YYYY-MM-DD or YYYYMMDD",
            );
        }
    }

    #[test]
    fn rejects_invalid_explicit_named_stores_for_every_command() {
        let raw = r#"{"pi":{"stores":[{"name":"omp"}]}}"#;
        for command in [vec!["daily"], vec!["blocks"], vec!["codex", "daily"]] {
            let config = config_context_for_command(raw, &command);
            assert_config_error(&config, "pi.stores[0]: missing field `path`");
        }
    }

    #[test]
    fn normalizes_valid_config_date_bounds() {
        let config = config_context_from_json(
            r#"{ "defaults": { "since": "2026-01-01", "until": "20260131" } }"#,
        );
        let mut shared = SharedArgs::default();

        assert_eq!(config.config_error(), None);
        apply_config_to_shared(&mut shared, &config);

        assert_eq!(shared.since.as_deref(), Some("20260101"));
        assert_eq!(shared.until.as_deref(), Some("20260131"));
    }

    fn user_config_timezone() -> Option<String> {
        let config = ConfigContext::from_args(&["daily".to_string()]);
        let mut shared = SharedArgs::default();
        apply_config_to_shared(&mut shared, &config);
        shared.timezone
    }

    #[test]
    fn loads_the_user_config_from_xdg_config_home() {
        let fixture = fs_fixture!({
            "xdg/claude/ccusage.json": r#"{ "defaults": { "timezone": "Asia/Tokyo" } }"#,
        });
        let _env = EnvVarsGuard::set_many([
            ("CLAUDE_CONFIG_DIR", None),
            ("HOME", Some(fixture.path("home").into())),
            ("XDG_CONFIG_HOME", Some(fixture.path("xdg").into())),
        ]);

        assert_eq!(user_config_timezone().as_deref(), Some("Asia/Tokyo"));
    }

    #[test]
    fn loads_the_user_config_from_a_tilde_claude_config_dir_entry() {
        let fixture = fs_fixture!({
            "home/claude-a/.keep": "",
            "home/claude-b/ccusage.json": r#"{ "defaults": { "timezone": "Asia/Tokyo" } }"#,
        });
        let _env = EnvVarsGuard::set_many([
            ("HOME", Some(fixture.path("home").into())),
            ("CLAUDE_CONFIG_DIR", Some("~/claude-a, ~/claude-b".into())),
        ]);

        assert_eq!(user_config_timezone().as_deref(), Some("Asia/Tokyo"));
    }

    #[test]
    fn loads_the_user_config_beside_a_projects_claude_config_dir_entry() {
        let fixture = fs_fixture!({
            "archive/ccusage.json": r#"{ "defaults": { "timezone": "Asia/Tokyo" } }"#,
            "archive/projects/project/session.jsonl": "",
        });
        let _env = EnvVarsGuard::set_many([(
            "CLAUDE_CONFIG_DIR",
            Some(fixture.path("archive/projects").into()),
        )]);

        assert_eq!(user_config_timezone().as_deref(), Some("Asia/Tokyo"));
    }

    fn assert_config_error(config: &ConfigContext, detail: &str) {
        let error = config
            .config_error()
            .expect("invalid explicit config must fail");
        assert!(error.starts_with("Invalid ccusage config: "), "{error}");
        assert!(error.contains("ccusage.json:"), "{error}");
        assert!(error.ends_with(detail), "{error}");
    }

    fn context(value: Value, raw: &str, agent: Option<&str>, report: &str) -> ConfigContext {
        let parsed = crate::layers::ConfigFile::parse(value.clone());
        let (layers, error, warnings) = match parsed {
            Ok(file) => (file.layers, None, file.warnings),
            Err(error) => (BTreeMap::new(), Some(config_error(error)), Vec::new()),
        };
        ConfigContext {
            layers,
            command: ConfigCommand {
                raw: raw.to_string(),
                agent: agent.map(ToString::to_string),
                report: report.to_string(),
            },
            pi_stores: Vec::new(),
            error,
            warnings,
        }
    }

    fn config_context_from_json(raw: &str) -> ConfigContext {
        config_context_for_command(raw, &["daily"])
    }

    fn config_context_for_command(raw: &str, command: &[&str]) -> ConfigContext {
        let fixture = fs_fixture!({
            "ccusage.json": raw,
        });
        let mut args = command
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<String>>();
        args.push("--config".to_string());
        args.push(fixture.path("ccusage.json").to_string_lossy().into_owned());
        ConfigContext::from_args(&args)
    }
}
