use std::{ffi::OsString, num::NonZeroU32};

use serde_json::{Value, json};

use crate::help::{help_text, help_text_for_args};
use crate::*;
use ccusage_cli::*;
use ccusage_test_support::fs_fixture;

mod agents;
mod config_files;
mod dates;
mod help;
mod pi_stores;
mod precedence;
mod sections;
mod snapshots;
mod statusline;
mod telemetry;
mod validation;
mod week_start;

fn parse(args: &[&str]) -> Command {
    parse_with_config(args, &NoConfig)
}

fn parse_with_config(args: &[&str], config: &dyn CliConfig) -> Command {
    parse_from_with_config(args.iter().map(OsString::from), config).unwrap()
}

fn parse_error(args: &[&str]) -> String {
    match parse_from_with_config(args.iter().map(OsString::from), &NoConfig) {
        Ok(_) => panic!("expected parse error"),
        Err(error) => error,
    }
}

#[derive(Default)]
struct TestConfig {
    shared_json: Option<bool>,
    shared_order: Option<SortOrder>,
    shared_since: Option<&'static str>,
    shared_timezone: Option<&'static str>,
    shared_compact: Option<bool>,
    weekly_start: Option<WeekDay>,
    blocks_active: Option<bool>,
    blocks_token_limit: Option<&'static str>,
    blocks_session_length: Option<f64>,
    statusline_visual_burn_rate: Option<VisualBurnRate>,
    statusline_cost_source: Option<CostSource>,
    statusline_refresh_interval: Option<u64>,
    codex_speed: Option<CodexSpeed>,
    pi_path: Option<&'static str>,
    open_claw_path: Option<&'static str>,
}

impl CliConfig for TestConfig {
    fn timezone(&self) -> Option<&str> {
        self.shared_timezone
    }

    fn apply_shared(&self, shared: &mut SharedArgs) {
        if let Some(json) = self.shared_json {
            shared.json = json;
        }
        if let Some(order) = self.shared_order {
            shared.order = Some(order);
        }
        if let Some(since) = self.shared_since {
            shared.since = Some(since.parse().unwrap());
        }
        if let Some(compact) = self.shared_compact {
            shared.compact = compact;
        }
    }

    fn apply_weekly_args(&self, args: &mut WeeklyArgs) {
        if let Some(start_of_week) = self.weekly_start {
            args.start_of_week = start_of_week;
        }
    }

    fn apply_blocks_args(&self, args: &mut BlocksArgs) {
        if let Some(active) = self.blocks_active {
            args.active = active;
        }
        if let Some(token_limit) = self.blocks_token_limit {
            args.token_limit = Some(token_limit.parse().unwrap());
        }
        if let Some(session_length) = self.blocks_session_length {
            args.session_length = ccusage_cli::SessionLength::from_hours(session_length).unwrap();
        }
    }

    fn apply_statusline_args(&self, args: &mut StatuslineArgs) {
        if let Some(visual_burn_rate) = self.statusline_visual_burn_rate {
            args.visual_burn_rate = visual_burn_rate;
        }
        if let Some(cost_source) = self.statusline_cost_source {
            args.cost_source = cost_source;
        }
        if let Some(refresh_interval) = self.statusline_refresh_interval {
            args.refresh_interval = refresh_interval;
        }
    }

    fn agent_options(&self, agent: &str) -> AgentOptions {
        match agent {
            "codex" => AgentOptions {
                codex_speed: self.codex_speed.unwrap_or_default(),
                ..AgentOptions::default()
            },
            "pi" => AgentOptions {
                pi_path: self.pi_path.map(str::to_string),
                ..AgentOptions::default()
            },
            "openclaw" => AgentOptions {
                open_claw_path: self.open_claw_path.map(str::to_string),
                ..AgentOptions::default()
            },
            _ => AgentOptions::default(),
        }
    }
}
