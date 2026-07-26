# ccusage-cli

The plain argument types the runtime shares: `SharedArgs`, the per-command
argument structs, and the enums the reports switch on.

This crate deliberately has no dependencies, no build script, and no embedded
assets. `ccusage-core` and all 15 adapters depend on it, so anything heavier
would sit on every crate's critical path — that is why the parser, the help
renderer, and the help JSON live in `ccusage-cli-parser` instead.

## Public surface

- `types::AgentCommandArgs`
- `types::AgentReportKind`
- `types::BlocksArgs`
- `types::CliConfig`
- `types::CodexSpeed`
- `types::Command`
- `types::CostMode`
- `types::CostSource`
- `types::DailyArgs`
- `types::NamedPiStore`
- `types::NoConfig`
- `types::OPENCODE_AGENT_REPORTS`
- `types::PricingOverride`
- `types::STANDARD_AGENT_REPORTS`
- `types::SessionArgs`
- `types::SharedArgs`
- `types::SortOrder`
- `types::StatuslineArgs`
- `types::VisualBurnRate`
- `types::WeekDay`
- `types::WeeklyArgs`
- `types::normalize_date_bound`

## Depends on

No dependencies.

## Build layer

Built in the `foundation` Crane artifact layer, so a change here recompiles every adapter. It is small and dependency-free, so that is cheap.
