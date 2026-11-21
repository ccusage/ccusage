# ccusage-adapter-qwen

The Qwen Code adapter: it turns project and chat JSONL files
into the usage entries the reports render.

## Data source

- `${QWEN_DATA_DIR:-~/.qwen}/projects/<project>/chats/*.jsonl`
- `${QWEN_DATA_DIR:-~/.qwen}/projects/<project>/chats/archive/*.jsonl`, where Qwen Code moves
  archived sessions. A record present in both locations counts once.

`cachedContentTokenCount` is part of `promptTokenCount`. OpenAI-compatible
providers include `thoughtsTokenCount` in `candidatesTokenCount`. Gemini-style
records can store it separately. The share of the recorded total left after
prompt and candidate tokens bounds how much separately recorded thinking is
added for pricing.
When the candidate count is absent and the prompt count is recorded, the
difference between the total and prompt counts is output including thinking.

Each assistant usage record must carry a valid timestamp. Records without one
are skipped with an invalid-timestamp diagnostic instead of using the chat
file's modification time.

## Build layer

Built in the `adapters` Crane artifact layer.
