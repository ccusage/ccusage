# Muse Code record shapes

Muse Code stores one append-only, event-sourced `session.jsonl` per session
under `sessions/YYYY/MM/DD/<session-uuid>/`, with child-agent logs under
`subagent/<child-uuid>/`. There is no committed schema from Meta; the wire
format below is what the adapter parses.

## Discovery

Logs are found under `$XDG_DATA_HOME/muse/sessions/` (default
`~/.local/share/muse/sessions/`) on every OS; macOS additionally scans
`~/Library/Application Support/muse/sessions/` and Windows `%APPDATA%\muse\sessions\`
as defensive candidates. Muse Code currently ships Linux and macOS builds
only. See `paths.rs` for the root list.

## Envelope

Every line is one envelope:

```json
{"schema_version":1,"id":"<record-uuid>","stream":{"kind":"session","id":"<session-uuid>"},
 "sequence":89,"recorded_at":1785962540739784,"record_type":"event",
 "durability":"durable","causation_id":null,
 "payload_type":"runtime.session","payload_schema_version":1,"payload":{…}}
```

- `recorded_at` is **microseconds** since the Unix epoch; the adapter divides
  by 1000 to get the millisecond timestamps reports use.
- `sequence` restarts per turn and record `id`s repeat across sessions, so the
  dedupe identity is the `(stream.id, id)` pair.
- Rare tombstone lines carry no `payload_type` and are skipped.

## Records the adapter reads

| payload_type | payload | used for |
|---|---|---|
| `runtime.session.metadata` | `record.workspace_root` | project name (basename) |
| `runtime.session` (`event.kind == "model_completed"`) | `event.model`, `event.usage` | one usage entry per model call |

## Token semantics

`model_completed.usage` has exactly six fields:

- `input_tokens` — **gross, includes `cache_read_tokens`** (netted before storing)
- `cache_read_tokens` — cached prefix replayed this turn (`cached_tokens` is a
  duplicate fallback)
- `cache_write_tokens` — cache-creation tokens
- `output_tokens` — gross; `reasoning_tokens` is a subset of it and is priced at
  the output rate like every other adapter

## Cost

Muse logs no cost and Meta publishes no per-token rate card, so entries are
priced from the shared pricing map by model name. Unknown models report $0 and
the standard missing-pricing warning; users can add pricing overrides in
ccusage config.
