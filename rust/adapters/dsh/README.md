# ccusage-adapter-dsh

The DeepSeek Harness adapter reads per-session JSONL histories and converts
assistant message usage into ccusage entries.

## Data source

Reads `${DSH_HOME:-~/.dsh}/sessions/**/session.jsonl` and versioned
`session.vN.jsonl` files, including `.zstd` compressed files. A blank
`DSH_HOME` uses the default home directory.

The adapter uses the session header's id, optional working directory, and
format version. It counts `assistant/message` events and skips streaming
`assistant/chunk` events. Input, output, cache read, and cache write token counts
are kept in their corresponding fields; reported input tokens exclude cache
tokens.

Direct DeepSeek models use ccusage's timestamped DeepSeek pricing. Other
providers retain their provider-qualified model identity; exact provider pricing
is preferred, with narrowly recognized DeepSeek model aliases as a static
fallback.

## Public surface

- `load_entries`
- `summarize_entries`
- `run`

## Depends on

- `ccusage-adapter-common`
- `ccusage-core`
- `jiff`
- `serde`
- `serde_json`
- `zstd`
