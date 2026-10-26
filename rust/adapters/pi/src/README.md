# Pi-Agent and Oh My Pi Sources

Data sources:

```text
pi-agent: ${PI_AGENT_DIR:-~/.pi/agent/sessions/}
Oh My Pi (unified reports): ~/.omp/agent/sessions/
```

Unified reports also discover Oh My Pi sessions automatically as agent `omp`; raw model labels use the `[omp] ` prefix. This does not add a `ccusage omp` command: `ccusage pi ...` remains focused on pi-agent.

`PI_CONFIG_DIR` defaults to `.omp` and is joined under `$HOME`; a leading slash in its value does not make it absolute. Unified reports include the default agent directory at `<config-root>/agent/sessions/`. `PI_CODING_AGENT_DIR` adds another agent root, with `sessions/` appended; relative values resolve from the current working directory. Existing profile roots under `<config-root>/profiles/<name>/agent/sessions/` are included. On Linux and macOS, an existing `$XDG_DATA_HOME/omp` also contributes `sessions/` and existing `profiles/<name>/sessions/` roots, even when `PI_CODING_AGENT_DIR` is set.

The OMP reader supports local JSONL sessions, including the title slot before the header, top-level billable `model_usage` entries, `activeLeafId`, and opaque ID or path values in `parentSession`. Compaction `tokensBefore` is context metadata and is excluded from billable usage. SQLite or Redis-backed session storage and compressed `.jsonl.gz` archives are not read.

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
tree-format sessions, the parent candidate follows the root-to-leaf path selected
by `activeLeafId` or the final physical entry rather than physical JSONL order;
abandoned sibling and disconnected-root branches stay counted in the parent.
OMP fork/move copies can also match the full physical parent usage prefix, but
only when stable record IDs and usage signatures match; the longer proven prefix
is used. Both candidates consider only parent usage at or before the child fork
timestamp. The loader removes only a leading matching prefix, and only when the
parent file was discovered in the same store. Signatures include provider, API,
role, timestamp, model, all token fields, the effective total-token fallback,
and the effective billed cost in Display or Auto mode. The first mismatch and
all later records remain in the child session. Missing, malformed,
self-referential, or cyclic lineage is left unchanged so usage is not discarded
speculatively.

pi-subagents writes derived debug transcripts under `subagent-artifacts/`
directories inside the sessions tree. Those transcripts duplicate calls already
recorded in the primary session files, so the loader skips any `.jsonl` file
with a `subagent-artifacts/` path segment. `run-*/` fresh-context child
sessions are the primary record for those children and keep counting.
