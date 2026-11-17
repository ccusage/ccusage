# ccusage-adapter-qwen

The Qwen Code adapter: it turns project and chat JSONL files
into the usage entries the reports render.

## Owns

- `loader.rs` — reading the source, dedupe, and date filtering.
- `parser.rs` — raw record parsing, token mapping, and model naming.
- `paths.rs` — environment variables, default directories, and file discovery.

Anything that is not specific to this source belongs in `ccusage-core` or
`ccusage-adapter-common` instead.

## Data source

- `${QWEN_DATA_DIR:-~/.qwen}/projects/<project>/chats/*.jsonl`
- `${QWEN_DATA_DIR:-~/.qwen}/projects/<project>/chats/archive/*.jsonl`, where Qwen Code moves
  archived sessions. A record present in both locations counts once.

Reads plain files through `ccusage-adapter-common`, which handles walking, size-balanced
chunking, and ordered parallel reads.

Each assistant usage record must carry a valid timestamp. Records without one
are skipped with an invalid-timestamp diagnostic instead of using the chat
file's modification time.

## Public surface

- `SOURCE`
- `summarize_entries`
- `run`
- `has_data`

## Depends on

- `ccusage-adapter-common`
- `ccusage-core`
- `jiff`
- `serde`
- `serde_json`

## Build layer

Built in the `adapters` Crane artifact layer; the layer compiles all adapters in one Cargo invocation, so they build concurrently.
