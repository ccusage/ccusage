# ccusage-adapter-gemini

The Gemini CLI adapter: it turns chat JSON and JSONL files
into the usage entries the reports render.

## Data source

- `${GEMINI_DATA_DIR:-~/.gemini}/tmp/**/chats/*.{json,jsonl}`

JSONL usage records without a valid timestamp are skipped with a diagnostic.
Whole-file JSON documents can use their session timestamp or file modification time.

## Build layer

Built in the `adapters` Crane artifact layer.
