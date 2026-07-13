---
name: agent-sources
description: Guides ccusage agent source formats. Use when checking agent log locations, raw record structure, token mappings, model names, precomputed costs, or source-specific CLI behavior.
---

# ccusage Agent Sources

Use this skill when inspecting source data formats, log paths, token
normalization, precomputed cost semantics, or source-specific command behavior
for any supported agent.

## Shared Report Concepts

Reports aggregate raw usage into daily, monthly, session, or billing-block summaries and output either tables or JSON.

The canonical command surface is the unified `ccusage` CLI:

```sh
ccusage daily
ccusage codex daily
ccusage opencode daily
ccusage amp daily
ccusage pi daily
```

Standalone agent wrapper packages have been removed. Use the unified `ccusage <agent> ...` commands in docs, tests, and examples, and do not reintroduce wrapper commands such as `ccusage-codex`, `ccusage-opencode`, `ccusage-amp`, or `ccusage-pi`.

Cost modes:

- `auto` - prefer pre-calculated `costUSD` when available, otherwise calculate from tokens.
- `calculate` - calculate from token counts and ignore pre-calculated costs.
- `display` - use pre-calculated costs and show `0` when missing.

Pricing generally comes from LiteLLM's `model_prices_and_context_window.json`. The `--offline` flag forces embedded pricing snapshots where supported.

## Agent Details

Read only the relevant adapter README before changing parser behavior, token
mappings, data directory detection, fallback models, or agent-specific CLI
flags:

- Claude Code: `rust/crates/ccusage/src/adapter/claude/README.md`
- Codex: `rust/crates/ccusage/src/adapter/codex/README.md`
- OpenCode: `rust/crates/ccusage/src/adapter/opencode/README.md`
- Amp: `rust/crates/ccusage/src/adapter/amp/README.md`
- pi-agent: `rust/crates/ccusage/src/adapter/pi/README.md`
- Grok Build CLI: `rust/crates/ccusage/src/adapter/grok/README.md`

## Implementation Notes

Agent adapter architecture lives in
`rust/crates/ccusage/src/adapter/AGENTS.md`. Read that local architecture file
when changing adapter module layout, shared implementation boundaries, migration
strategy, tests, docs, terminal output, or benchmark expectations.

Keep command names and flag semantics aligned across agents unless the source
data forces a difference.
