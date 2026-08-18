# ccusage-adapter-all

The unified `ccusage` report: it loads every agent adapter and merges their rows
into one table or JSON document.

## Owns

- `loader.rs` — per-agent loading, the agent selection the CLI asks for, and the
  merge into shared rows.
- `report.rs` — the unified row and total shapes.
- `types.rs` — the accumulators the merge needs.

This is the only crate that depends on all 17 adapters, which keeps the adapters
themselves independent of each other.

## Public surface

- `run`

## Depends on

- `ccusage-adapter-amp`
- `ccusage-adapter-claude`
- `ccusage-adapter-codebuff`
- `ccusage-adapter-codex`
- `ccusage-adapter-common`
- `ccusage-adapter-copilot`
- `ccusage-adapter-droid`
- `ccusage-adapter-gemini`
- `ccusage-adapter-goose`
- `ccusage-adapter-grok`
- `ccusage-adapter-zcode`
- `ccusage-adapter-hermes`
- `ccusage-adapter-kilo`
- `ccusage-adapter-kimi`
- `ccusage-adapter-openclaw`
- `ccusage-adapter-opencode`
- `ccusage-adapter-pi`
- `ccusage-adapter-qwen`
- `ccusage-cli`
- `ccusage-core`
- `serde`
- `serde_json`

## Build layer

Built in the `adapters` Crane artifact layer; the layer compiles all adapters in one Cargo invocation, so they build concurrently. Because it depends on every adapter, a change to any adapter also
recompiles this crate.
