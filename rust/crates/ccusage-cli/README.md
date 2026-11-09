# ccusage-cli

The plain argument types the runtime shares: `SharedArgs`, the per-command
argument structs, the enums the reports switch on, and the few constants and
helpers (`BUILT_IN_AGENT_NAMES`, `DEFAULT_SESSION_DURATION_HOURS`,
`home::home_dir`) that `ccusage-config` and `ccusage-cli-parser` need without
depending on `ccusage-core`.

This crate deliberately has no dependencies, no build script, and no embedded
assets. `ccusage-core` and all 17 adapters depend on it, so anything heavier
would sit on every crate's critical path — that is why the parser, the help
renderer, and the help JSON live in `ccusage-cli-parser` instead.

## Public surface

- `BUILT_IN_AGENT_NAMES`
- `DEFAULT_SESSION_DURATION_HOURS`
- `home::home_dir`
- `types::AgentCommandArgs`
- `types::AgentReportKind`
- `types::BlocksArgs`
- `types::CliConfig`
- `types::CodexSpeed`
- `types::Command`
- `types::CostMode`
- `types::CostSource`
- `types::DATE_BOUND_FORMATS`
- `types::DailyArgs`
- `types::NamedPiStore`
- `types::NoConfig`
- `types::OPENCODE_AGENT_REPORTS`
- `types::PricingOverride`
- `types::STANDARD_AGENT_REPORTS`
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
