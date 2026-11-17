---
name: agent-sources
description: Guides ccusage agent source formats. Use when checking where an agent stores its logs, how raw records map to tokens and models, how precomputed costs interact with cost modes, or which reports an agent supports.
---

# ccusage Agent Sources

Every agent is an adapter crate in `rust/adapters/<agent>` that turns one
source's raw logs into the shared report shapes. `ccusage <report>` aggregates
all of them; `ccusage <agent> <report>` scopes to one.

Keep command names and flag semantics aligned across agents unless the source
data forces a difference. Declare supported reports in the agent table in
`rust/crates/ccusage-cli/src/agents.rs`.

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
