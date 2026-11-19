# ccusage-adapter-qwen

The Qwen Code adapter: it turns project and chat JSONL files
into the usage entries the reports render.

## Data source

- `${QWEN_DATA_DIR:-~/.qwen}/projects/<project>/chats/*.jsonl`
- `${QWEN_DATA_DIR:-~/.qwen}/projects/<project>/chats/archive/*.jsonl`, where Qwen Code moves
  archived sessions. A record present in both locations counts once.

Each assistant usage record must carry a valid timestamp. Records without one
are skipped with an invalid-timestamp diagnostic instead of using the chat
file's modification time.

## Build layer

Built in the `adapters` Crane artifact layer.
