# ccusage-cli

The plain argument types the runtime shares: `SharedArgs`, the per-command
argument structs (including an optional requested sort order), the enums the reports switch on, the built-in agent table
(`AGENTS`: each agent's name, label and reports), and the few constants and
helpers (`DEFAULT_SESSION_DURATION_HOURS`,
`home::home_dir`, `home::expand_home_path`, `home::claude_config_dirs`) that
`ccusage-config` and `ccusage-cli-parser` need without depending on
`ccusage-core`. `home::claude_config_dirs` is the one place the Claude Code
directories are resolved, so the Claude adapter and user config discovery
agree.

This crate deliberately has no dependencies, no build script, and no embedded
assets. `ccusage-core` and every adapter depend on it, so anything heavier
would sit on every crate's critical path — that is why the parser, the help
renderer, and the help JSON live in `ccusage-cli-parser` instead.

## Public surface

- `agents::AGENTS`
- `agents::AgentId`
- `agents::AgentSpec`
- `agents::BUILT_IN_AGENT_NAMES`
- `agents::StandardAgent`
- `DEFAULT_SESSION_DURATION_HOURS`
- `home::ClaudeConfigDirs`
- `home::claude_config_dirs`
- `home::expand_home_path`
- `home::home_dir`
- `is_shared_flag`
- `option_takes_value`
- `types::AgentCommandArgs`
- `types::AgentOptions`
- `types::AgentReportKind`
- `types::BlocksArgs`
- `types::CliConfig`
- `types::CodexSpeed`
- `types::ColorChoice`
- `types::Command`
- `types::CostMode`
- `ContextThresholds`
- `SessionLength`
- `TokenLimit`
- `types::CostSource`
- `types::DATE_BOUND_FORMATS`
- `types::DailyArgs`
- `types::NamedPiStore`
- `types::NoConfig`
- `types::PeriodUnit`
- `types::PricingOverride`
- `types::SessionArgs`
- `types::SharedArgs`
- `types::SortOrder`
- `types::SourceArgs`
- `types::StatuslineArgs`
- `types::VisualBurnRate`
- `types::WeekDay`
- `types::WeeklyArgs`
- `types::normalize_date_bound`

## Depends on

No dependencies.

## Build layer

Built in the `foundation` Crane artifact layer, so a change here recompiles every adapter. It is small and dependency-free, so that is cheap.
