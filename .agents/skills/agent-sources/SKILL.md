---
name: agent-sources
description: Guides ccusage agent source formats. Use when checking where an agent stores its logs, how raw records map to tokens and models, how precomputed costs interact with cost modes, or which reports an agent supports.
---

# ccusage Agent Sources

Read `rust/adapters/<agent>/README.md` and, where present, `src/README.md`
for source paths, record shapes, and accounting rules. Follow the adapter
and pricing candidate boundaries in `rust/adapters/README.md` and the
change workflow in `rust/adapters/AGENTS.md`.

Keep command and option semantics aligned across agents. Declare supported
reports in `rust/crates/ccusage-cli/src/agents.rs`. Preserve source-specific
stored-cost, reasoning-token, and residual-token policies. Follow the cost-mode
contract in `docs/guide/cost-modes.md` and reuse
`rust/crates/ccusage-core/src/cost.rs` when it preserves those policies.
