# AGENTS.md - Agent Source Architecture

`README.md` covers adapter architecture, the shared-vs-source boundary, and the crate module shape;
`rust/adapters/opencode/src/` is a compact example of it. This file adds the workflow rules for
changing or adding an adapter.

- Moving a loader into an adapter updates internal imports to the adapter path rather than leaving
  compatibility re-export shims behind. Root-level modules stay only when they are declared public
  exports or dedicated packaging entries.
- Detection short-circuits as soon as one usable source file is found.
- Cover path discovery, parser behavior, aggregation totals, and legacy compatibility with
  fixture-backed Rust tests, plus CLI JSON assertions and table snapshots for every affected report
  mode. Skipped local-data smoke tests against real user log directories are welcome for catching
  schema drift.
- A new agent guide lands with its README usage examples, docs guide, related-guide links, and
  VitePress navigation in the same change unless the user scopes documentation out; use the `docs`
  skill to find every entrypoint that lists supported agents.
- Table layout, progress, or spinner changes get verified with `cmux-debug`; performance-sensitive
  changes get a `profile` comparison against main or the previous tag, recording whether JSON output
  still matches over the comparison window.
