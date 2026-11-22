use ccusage_cli::{
    AgentOptions, BlocksArgs, ColorChoice, ContextThresholds, DailyArgs, ReportTimezone,
    SharedArgs, StatuslineArgs, WeeklyArgs,
};

use super::{ConfigContext, pricing::merge_pricing_overrides};
use crate::config_schema::SharedOptions;

pub(super) fn apply_config_to_shared(shared: &mut SharedArgs, config: &ConfigContext) {
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
        if let Some(project_aliases) = options.project_aliases {
            args.project_aliases = Some(project_aliases);
        }
    }
}

pub(super) fn apply_config_to_weekly_args(args: &mut WeeklyArgs, config: &ConfigContext) {
    for options in config.option_maps() {
        if let Some(day) = options.weekly.clone().start_of_week {
            args.start_of_week = day.into();
        }
    }
}

pub(super) fn apply_config_to_blocks_args(args: &mut BlocksArgs, config: &ConfigContext) {
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

pub(super) fn apply_config_to_statusline_args(args: &mut StatuslineArgs, config: &ConfigContext) {
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
        if agent == "claude" {
            resolved.claude_project = self
                .option_maps()
                .into_iter()
                .rev()
                .find_map(|options| options.claude.project.as_ref())
                .cloned();
            return resolved;
        }
        for options in self.option_maps() {
            match agent {
                "codex" => {
                    if let Some(speed) = options.codex.clone().speed {
                        resolved.codex_speed = speed.into();
                    }
                }
                "pi" => {
                    let pi = options.pi.clone();
                    if let Some(path) = pi.pi_path {
                        resolved.pi_path = Some(path);
                    }
                    if let Some(path) = pi.pi_agent_dir {
                        resolved.pi_agent_dir = Some(path);
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
