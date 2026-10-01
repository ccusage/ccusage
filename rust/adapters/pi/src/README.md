# pi-agent Source

Data source:

```text
${PI_AGENT_DIR:-~/.pi/agent/sessions/}
```

Commands:

```sh
ccusage pi daily
ccusage pi monthly
ccusage pi session
ccusage pi daily --json
ccusage pi daily --pi-path /path/to/sessions
```

Cost rules: `--mode display` uses the `usage.cost.total` value from the session.
In the default `auto` mode, an explicit `pricingOverrides` entry for the
prefixed or bare model name recalculates cost from tokens, even when the session
contains a display cost of zero. Without an override, Auto keeps a valid stored
cost and calculates from tokens only when that cost is missing or invalid.

Forked session files may replay the usage history of their parent. For Pi's
tree-format sessions, the parent candidate follows the root-to-leaf path ending
at the final physical entry rather than physical JSONL order; abandoned sibling
and disconnected-root branches stay counted in the parent. The candidate includes
only raw records at or before the child fork timestamp. The loader removes only a
leading prefix that matches that candidate,
and only when the parent file was discovered in the same store. Matching
includes the timestamp, model, all token fields, the effective total-token
fallback, and the effective billed cost in Display or Auto mode. The first
mismatch and all later records remain in the child session. Missing, malformed,
self-referential, or cyclic lineage is left unchanged so usage is not discarded
speculatively.

pi-subagents writes derived debug transcripts under `subagent-artifacts/`
directories inside the sessions tree. Those transcripts duplicate calls already
recorded in the primary session files, so the loader skips any `.jsonl` file
with a `subagent-artifacts/` path segment. `run-*/` fresh-context child
sessions are the primary record for those children and keep counting.
