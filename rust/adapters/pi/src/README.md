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
