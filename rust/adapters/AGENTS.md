# AGENTS.md - Agent Source Architecture

This directory wires the native `ccusage` binary to the adapter crates under
`rust/adapters/<agent>`.

Read `README.md` first for adapter architecture, module layout, and
source-specific README conventions. This file adds agent workflow rules for
changes under `rust/adapters/<agent>/`.

When moving an existing loader into an adapter, update internal imports to the
adapter path instead of adding compatibility re-export shims. Keep old root-level
modules only when they are part of the package's declared public exports or are
dedicated packaging entries.

## Migration Checklist

For each migrated or new agent:

- Put all source-specific runtime logic under
  `rust/adapters/<agent>/src/`.
- Implement fast detection that short-circuits once a usable source file is found.
- Use shared file walking, JSONL scanning where applicable, SQLite loading,
  logging, pricing fetcher lifecycle, date formatting, table rendering, and
  all-agent aggregation.
- Keep adapter code responsible for source paths, raw parsing, token mapping, model mapping, source metadata, and agent-specific pricing.
- Add Rust fixture-backed tests for path discovery, parser behavior, aggregation totals, and important legacy compatibility.
- Add skipped local-data smoke tests when real user log directories are useful for catching schema drift.
- Add or update CLI JSON assertions and table snapshots for affected report modes.
- Audit every user-facing entrypoint that lists supported agents, commands, options, report modes, or examples. Update docs when the adapter changes what users can run or discover; root `AGENTS.md` owns the cross-repository docs update rule.
- When adding a new agent guide, include README usage examples, docs guide content, related guide links, and VitePress navigation in the same change unless the user explicitly scopes documentation out.
- Validate terminal output with `cmux-debug` when changing table layout, progress, spinners, or responsive behavior.
- Benchmark affected agents against main or the previous tag, and record whether JSON output still matches for the comparison window.
