# ccusage-adapter-gemini

The Gemini CLI adapter: it turns chat JSON and JSONL files
into the usage entries the reports render.

## Data source

- `${GEMINI_DATA_DIR:-~/.gemini/tmp}`, scanned recursively for `*.json` and `*.jsonl` files

`GEMINI_DATA_DIR` replaces the default `~/.gemini/tmp` root and accepts a comma-separated list of
roots. Every JSON and JSONL file below a root is a candidate: discovery does not require a `tmp` or
`chats` path component. Gemini CLI normally writes `~/.gemini/tmp/*/chats/*.{json,jsonl}`, so a
relocated layout sets the variable to the equivalent of `~/.gemini/tmp`, not to the directory above
it.

JSONL usage records without a valid timestamp are skipped with a diagnostic.
Whole-file JSON documents can use their session timestamp or file modification time.

## Build layer

Built in the `adapters` Crane artifact layer.
