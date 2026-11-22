# ccusage-adapter-dsh

The DeepSeek Harness (`dsh`) adapter: it turns the local session logs that DeepSeek Harness writes
into the usage entries the reports render. Support is experimental while the harness format
evolves.

## Data source

```text
${DSH_HOME:-~/.dsh}/sessions/<project>/<session-id>/session.jsonl
${DSH_HOME:-~/.dsh}/sessions/<project>/<session-id>/session.v<N>.jsonl
```

Either file can be zstd-compressed with a `.zstd` suffix. `DSH_HOME` may hold
comma-separated roots and replaces the default. Every root is checked and missing roots are
skipped. A request found in more than one root is counted once.

DeepSeek Harness keeps earlier log generations beside the newest one after a session migrates,
so the adapter reads only the highest `v<N>` generation in each session directory.
`session.jsonl` is generation zero. Compressed logs are appended one zstd frame per write batch,
so a torn final frame keeps the records before it. When the log does not change during the read,
the torn tail is reported as skipped input; a log that is still being written is only noted at
debug level.

## Record shape

The first non-blank line is a session header. Later lines are events:

```json
{"type":"session","version":4,"id":"sid-1","createdAt":1785015039278,"cwd":"/home/u/proj","isSeeded":false}
{"type":"request/header","seq":3,"time":1785015040000,"data":{"header":{"config":{"provider":"deepseek-official","model":"deepseek-v4-pro"}}}}
{"type":"assistant/message","seq":5,"time":1785015045000,"data":{"usage":{"inputTokens":1200,"outputTokens":300,"cacheReadTokens":8000,"totalTokens":9500}}}
```

The adapter reads these event types:

- `request/header` and `request/context` update the route used by later requests. A record without
  a model leaves the previous route in place. The harness records its own billing routes as
  `deepseek-official` and `deepseek-account`.
- `assistant/message` and `assistant/attempt` carry usage for one request. When the event has no
  settled `data.usage`, the adapter uses the last `usage` chunk in `data.stream`, as the harness
  meter itself does.
- `session/end-seed` with `data.inherited` marks the end of history copied from a parent session.
- `compaction/summary` carries its own `provider`, `model` and `usage`.

Usage counters are `inputTokens`, `outputTokens`, `cacheReadTokens`, `cacheWriteTokens` and
`totalTokens`. Counter fields are read leniently; an event without positive total usage is
ignored. Event `time` is epoch milliseconds and falls back to the header `createdAt`. A request
with neither is skipped with an invalid-timestamp diagnostic.

Event type matching tolerates escaped slashes (`request\/context`) because the line is decoded
before dispatch.

## Token and cost mapping

DeepSeek Harness emits disjoint counters: `inputTokens` already excludes cache reads and writes.
They map directly to input, output, cache read and cache write. A `totalTokens` value larger than
the sum of those buckets is preserved through the shared extra-token fallback, so the report
total matches the harness total.

When uncached input is recorded and output is absent, the exact full-call total can recover
output after subtracting uncached input and both cache buckets.

Assistant messages and attempts use the route from the most recent `request/header` or
`request/context`; compaction summaries use the provider and model they name. A request without a
model is reported as `unknown`, and a missing session ID falls back to the session directory name.

DeepSeek Harness records no cost. `auto` and `calculate` price the tokens from the shared pricing
map; `display` reports zero cost. First-party routes (`deepseek`, `deepseek-official`,
`deepseek-account`) look up the logged model and then `deepseek/<model>`, keeping DeepSeek's
peak and off-peak schedule. Other providers try `<provider>/<model>` and then the bare model, and
keep that flat rate even when the bare model matches a DeepSeek row. When no candidate has
pricing, the entry stays visible with the resolved model in the missing-pricing warning.

## Accounting rules

- A forked session copies its parent log up to an inherited `session/end-seed` marker. A fork can
  carry more than one inherited marker, so usage before the last one is discarded and only
  requests after it are counted. When the header sets `isSeeded` and the log ends before any
  inherited marker, the log is read as its parent's usage and dropped with a debug note; a log
  without `isSeeded` keeps its entries.
- Entries are deduplicated by session ID and `seq`. Events without `seq` fall back to the full
  timestamp, model and token tuple. Conflicting copies are ranked by billable token total,
  then by output tokens, then by their recorded fields to keep selection deterministic.
- Session rows use the session ID from the log header. The project path is the header `cwd`, or
  `DeepSeek Harness` when absent.

## Testing

```text
cargo test -p ccusage-adapter-dsh
```

The fixture tests cover zstd frames, torn and corrupt tails, generation selection, root
deduplication, route changes, stream fallbacks, fork seeding, diagnostics, pricing routes and the
focused report snapshots.

## Build layer

Built in the `adapters` Crane artifact layer.
