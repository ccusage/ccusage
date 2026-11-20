use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

use ccusage_cli::command_tokens;
use ccusage_cli::{
    AgentOptions, BlocksArgs, CodexSpeed, ColorChoice, ContextThresholds, CostMode, CostSource,
    DailyArgs, NamedPiStore, PricingOverride, ReportTimezone, SharedArgs, SortOrder,
    StatuslineArgs, VisualBurnRate, WeekDay, WeeklyArgs,
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
    pi_stores_error: Option<String>,
    error: Option<String>,
    layers: BTreeMap<String, crate::layers::Layer>,
    warnings: RefCell<Vec<String>>,
    config_origin: Option<(PathBuf, bool)>,
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
        let explicit_path = scan_config_path(args);
        let (file, error, warnings) = load_config_value(explicit_path.as_deref());
        let mut context = Self::empty(command);
        context.error = error;
        context.warnings = RefCell::new(warnings);
        if explicit_path.is_none()
            && context.command.report == "statusline"
            && let Some(error) = context.error.take()
        {
            context.warnings.get_mut().push(error);
        }
        if let Some(file) = file {
            context.pi_stores = file.pi_stores;
            context.pi_stores_error = file.pi_stores_error;
            context.layers = file.layers;
            context.config_origin = Some((file.path, explicit_path.is_some()));
        }
        context
    }

    pub fn warnings(&self) -> Vec<String> {
        self.warnings.borrow().clone()
    }

    fn empty(command: ConfigCommand) -> Self {
        Self {
            command,
            pi_stores: Vec::new(),
            pi_stores_error: None,
            error: None,
            layers: BTreeMap::new(),
            warnings: RefCell::new(Vec::new()),
            config_origin: None,
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
            pi_stores_error: self.pi_stores_error.clone(),
            error: self.error.clone(),
            layers: self.layers.clone(),
            warnings: self.warnings.clone(),
            config_origin: self.config_origin.clone(),
        }
    }

    fn resolve_context_thresholds(
        &self,
        low_override: Option<u8>,
        medium_override: Option<u8>,
    ) -> Result<ContextThresholds, String> {
        let mut lows = Vec::new();
        let mut mediums = Vec::new();
        for (index, path) in self.option_paths().into_iter().enumerate() {
            if let Some(layer) = self.layers.get(&path) {
                if let Some(value) = layer.statusline.context_low_threshold {
                    lows.push((index, path.clone(), value));
                }
                if let Some(value) = layer.statusline.context_medium_threshold {
                    mediums.push((index, path, value));
                }
            }
        }
        let defaults = ContextThresholds::default();
        loop {
            let low = low_override
                .or_else(|| lows.last().map(|(_, _, value)| *value))
                .unwrap_or(defaults.low());
            let medium = medium_override
                .or_else(|| mediums.last().map(|(_, _, value)| *value))
                .unwrap_or(defaults.medium());
            match ContextThresholds::new(low, medium) {
                Ok(thresholds) => {
                    return Ok(thresholds);
                }
                Err(error) => {
                    let drop_low = match (low_override, medium_override) {
                        (Some(_), None) if !mediums.is_empty() => false,
                        (None, Some(_)) if !lows.is_empty() => true,
                        (None, None) => lows.last().is_some_and(|(index, _, _)| {
                            mediums.last().is_none_or(|(other, _, _)| index >= other)
                        }),
                        _ => return Err(error),
                    };
                    let (field, invalid) = if drop_low {
                        ("contextLowThreshold", lows.pop())
                    } else {
                        ("contextMediumThreshold", mediums.pop())
                    };
                    let Some((_, path, _)) = invalid else {
                        return Err(config_error(error));
                    };
                    let Some((file, strict)) = &self.config_origin else {
                        return Err(error);
                    };
                    let message = format!("{}: {path}.{field}: {error}", file.display());
                    if *strict {
                        return Err(config_error(message));
                    }
                    self.warnings
                        .borrow_mut()
                        .push(format!("Invalid ccusage config field: {message}"));
                }
            }
        }
    }

    fn warn_discovered_statusline_field(&self, path: &str, field: &str, message: &str) -> bool {
        let Some((file, false)) = &self.config_origin else {
            return false;
        };
        self.warnings.borrow_mut().push(format!(
            "Invalid ccusage config field: {}: {path}.{field}: {message}",
            file.display()
        ));
        true
    }

    fn option_paths(&self) -> Vec<String> {
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
    }

    fn timezone_setting(&self) -> Option<(String, &str)> {
        self.option_paths().into_iter().rev().find_map(|path| {
            let layer = self.layers.get(&path)?;
            let value = if self.command.report == "statusline" {
                &layer.statusline.timezone
            } else {
                &layer.shared.timezone
            };
            Some((path, value.as_deref()?))
        })
    }

    fn option_maps(&self) -> Vec<&crate::layers::Layer> {
        if self.layers.is_empty() {
            return Vec::new();
        }
        self.option_paths()
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
        if !names.insert(name.clone()) {
            return Err(config_error(format!("duplicate pi.stores name '{name}'")));
        }
        parsed.push(NamedPiStore {
            name: name.clone(),
            path: path.clone(),
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
                return (None, Some(message), warnings);
            }
        };
        let value = serde_json::from_str::<Value>(&content)
            .map_err(|error| error.to_string())
            .and_then(|value| {
                if value.is_object() {
                    Ok(value)
                } else {
                    Err("$: expected an object".to_string())
                }
            });
        let value = match value {
            Ok(value) => value,
            Err(error) => {
                let message = config_error(format!("{}: {error}", path.display()));
                if explicit {
                    return (None, Some(message), warnings);
                }
                warnings.push(message);
                continue;
            }
        };
        let file = if explicit {
            crate::layers::ConfigFile::parse(value)
        } else {
            crate::layers::ConfigFile::parse_discovered(value)
        };
        match file {
            Ok(mut file) => {
                for layer in file.layers.values_mut() {
                    if let Some(error) = &mut layer.date_error {
                        *error = config_error(format!("{}: {error}", path.display()));
                    }
                }
                match parse_named_pi_stores(&file.value) {
                    Ok(stores) => file.pi_stores = stores,
                    Err(error) => {
                        let detail = error
                            .strip_prefix("Invalid ccusage config: ")
                            .unwrap_or(&error);
                        let message = config_error(format!("{}: {detail}", path.display()));
                        if explicit {
                            return (None, Some(message), warnings);
                        }
                        file.pi_stores_error = Some(message);
                    }
                }
                file.path = path.clone();
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
            args.token_limit = Some(token_limit);
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
    fn context_thresholds(
        &self,
        low: Option<u8>,
        medium: Option<u8>,
    ) -> Result<ContextThresholds, String> {
        self.resolve_context_thresholds(low, medium)
    }

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
        self.error
            .as_deref()
            .or_else(|| {
                (self.command.agent.is_none()
                    && matches!(
                        self.command.report.as_str(),
                        "daily" | "weekly" | "monthly" | "session"
                    ))
                .then_some(self.pi_stores_error.as_deref())
                .flatten()
            })
            .or_else(|| {
                (self.command.report != "statusline")
                    .then(|| {
                        self.option_maps()
                            .into_iter()
                            .find_map(|layer| layer.date_error.as_deref())
                    })
                    .flatten()
            })
    }

    fn timezone(&self) -> Option<&str> {
        self.timezone_setting().map(|(_, value)| value)
    }

    fn timezone_config_field(&self) -> Option<String> {
        let (file, _) = self.config_origin.as_ref()?;
        let (path, _) = self.timezone_setting()?;
        Some(format!("{}: {path}.timezone", file.display()))
    }

    fn recover_statusline_timezone(
        &self,
        rejected: &str,
        error: String,
    ) -> Result<Option<ReportTimezone>, String> {
        for path in self.option_paths().into_iter().rev() {
            let Some(value) = self
                .layers
                .get(&path)
                .and_then(|layer| layer.statusline.timezone.as_deref())
            else {
                continue;
            };
            if value != rejected
                && let Ok(timezone) = value.parse()
            {
                return Ok(Some(timezone));
            }
            let message = format!("unknown IANA timezone '{value}'");
            if !self.warn_discovered_statusline_field(&path, "timezone", &message) {
                return Err(error);
            }
        }
        Ok(None)
    }

    fn recover_statusline_window(
        &self,
        shared: &mut SharedArgs,
        since_flag: bool,
        until_flag: bool,
        message: &str,
    ) -> bool {
        let Some((file, false)) = &self.config_origin else {
            return false;
        };
        let paths = self.option_paths();
        let layers = paths
            .iter()
            .filter_map(|path| self.layers.get(path).map(|layer| (path, &layer.shared)))
            .collect::<Vec<_>>();
        let since_source = if since_flag {
            None
        } else {
            layers
                .iter()
                .rposition(|(_, options)| options.since.is_some())
        };
        let until_source = if until_flag {
            None
        } else {
            layers
                .iter()
                .rposition(|(_, options)| options.until.is_some())
        };
        let Some(first_source) = since_source.into_iter().chain(until_source).min() else {
            return false;
        };
        let since_path = since_source.map_or_else(
            || "--since".to_string(),
            |index| format!("{}.since", layers[index].0),
        );
        let until_path = until_source.map_or_else(
            || "--until".to_string(),
            |index| format!("{}.until", layers[index].0),
        );
        let warn = |field: &str, paired_with: &str, message: &str| {
            self.warnings.borrow_mut().push(format!(
                "Invalid ccusage config field: {}: {field}: {message} (paired with {paired_with})",
                file.display()
            ));
        };
        let (field, paired_with) = if since_flag {
            (&until_path, &since_path)
        } else {
            (&since_path, &until_path)
        };
        warn(field, paired_with, message);

        let fallback = layers[..first_source]
            .iter()
            .rev()
            .find_map(|(path, options)| {
                let since = &options.since.as_ref()?.0;
                let until = &options.until.as_ref()?.0;
                if since > until {
                    warn(
                        &format!("{path}.since"),
                        &format!("{path}.until"),
                        &format!(
                            "The --since date '{since}' is later than the --until date '{until}'."
                        ),
                    );
                    return None;
                }
                let compatible_with_flags = (!since_flag
                    || shared.since.as_ref().is_none_or(|flag| flag <= until))
                    && (!until_flag || shared.until.as_ref().is_none_or(|flag| since <= flag));
                compatible_with_flags.then_some((since, until))
            });
        if !since_flag {
            shared.since = fallback.map(|(since, _)| *since);
        }
        if !until_flag {
            shared.until = fallback.map(|(_, until)| *until);
        }
        true
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
mod tests;
