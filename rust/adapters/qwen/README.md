# ccusage-adapter-qwen

The Qwen Code adapter: it turns project and chat JSONL files
into the usage entries the reports render.

## Data source

- `${QWEN_DATA_DIR:-~/.qwen}/projects/<project>/chats/*.jsonl`
- `${QWEN_DATA_DIR:-~/.qwen}/projects/<project>/chats/archive/*.jsonl`, where Qwen Code moves
  archived sessions. A record present in both locations counts once.

`cachedContentTokenCount` is part of `promptTokenCount`, and output including
thinking is `totalTokenCount - promptTokenCount`, as in Qwen Code itself.
`thoughtsTokenCount` is never added on top: OpenAI-compatible providers count it
inside `candidatesTokenCount`. Without a total, thinking is taken to be inside
the candidates.

Each assistant usage record must carry a valid timestamp. Records without one
are skipped with an invalid-timestamp diagnostic instead of using the chat
file's modification time.

## Build layer

Built in the `adapters` Crane artifact layer.
