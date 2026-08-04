---
name: agent-sources
description: Guides ccusage agent source formats. Use when checking where an agent stores its logs, how raw records map to tokens and models, how precomputed costs interact with cost modes, or which reports an agent supports.
---

# ccusage Agent Sources

Every agent is an adapter crate in `rust/adapters/<agent>` that turns one
source's raw logs into the shared report shapes. `ccusage <report>` aggregates
all of them; `ccusage <agent> <report>` scopes to one.

Which reports an agent accepts is not uniform, and it is encoded twice:
`STANDARD_AGENT_REPORTS` / `OPENCODE_AGENT_REPORTS` in
`rust/crates/ccusage-cli/src/types.rs` drive parsing, while
`agent_report_supported` in `rust/crates/ccusage-cli-parser/src/parser.rs`
decides what is accepted per agent name. Changing an agent's report set means
updating both. Keep command names and flag semantics aligned across agents
unless the source data forces the difference, and say so in a comment when it
does.

Cost handling is shared, not per-agent: `auto`, `calculate`, and `display`
differ only in how a record's precomputed `costUSD` competes with token-derived
cost, decided in `rust/crates/ccusage-core/src/cost.rs`. Pricing comes from
LiteLLM with an embedded snapshot behind `--offline` (`pricing.rs`, and the
`rust` skill for how both snapshots are built into the binary).

## Before Changing A Source

Read that agent's own docs first — they carry the paths, record shapes, token
mapping, model fallbacks, and cost quirks that are not shared:

- `rust/adapters/<agent>/README.md` - what the crate owns and where its data lives.
- `rust/adapters/<agent>/src/README.md` - record shapes and token/cost rules,
  present for the sources that need them.

Architecture and the shared-vs-source boundary live in
`rust/adapters/README.md`; the per-change workflow in `rust/adapters/AGENTS.md`.
