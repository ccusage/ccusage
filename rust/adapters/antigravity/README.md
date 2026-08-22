# Antigravity Source

Tracks [Google Antigravity](https://antigravity.google) (agentic IDE + CLI,
Windsurf-derived) usage from local SQLite conversation databases.

Data source:

```text
${ANTIGRAVITY_DATA_DIR:-~/.gemini/antigravity}/conversations/<uuid>.db
~/.gemini/antigravity-cli/conversations/<uuid>.db
~/.gemini/antigravity-ide/conversations/<uuid>.db
~/.gemini/antigravity-backup/conversations/<uuid>.db
```

`ANTIGRAVITY_DATA_DIR` accepts one root or a comma-separated list of roots
and replaces the default `~/.gemini/antigravity*` set. Missing roots are
skipped. `*.db-wal`/`*.db-shm` sidecars and legacy `.pb` conversation files
(compressed/encrypted, unsupported) are ignored. A database without a
`gen_metadata` table is not an Antigravity conversation DB and yields nothing.

Commands:

```sh
ccusage antigravity daily
ccusage antigravity monthly
ccusage antigravity session
ccusage antigravity daily --json
```

## Protobuf field map

`gen_metadata.data` (ordered by `idx`) holds one `GeneratorMetadata` message
per generation, decoded with a hand-rolled wire-format reader (no .proto):

- field 1 (LEN message) → chatModel
  - field 19 (LEN string) → raw model id
  - field 9 (LEN message) → generation info; its field 4 → timestamp
    `{1: seconds varint, 2: nanos varint}` (nanos must be `0..=999_999_999`)
  - field 4 (LEN message) → usage
    - field 1 (varint) → fixed system-prompt input tokens
    - field 2 (varint) → fresh (non-cached) input tokens
    - field 5 (varint) → cache-read tokens
    - field 9 (varint) → output (text) tokens
    - field 10 (varint) → thinking/reasoning tokens
    - field 11 (LEN string) → responseId (dedup key, within and across DBs)

`trajectory_metadata_blob.data` (first row) holds session fallbacks:

- field 2 (LEN message) → `{1: seconds, 2: nanos}` session created-at
  (fallback when a generation has no timestamp; final fallback is file mtime)
- field 1 (LEN message) → field 1 (LEN string) → workspace `file://` URI
  (percent-decoded; project/session identifier)

## Token semantics

- `input_tokens` = system-prompt (#1) + fresh input (#2). The system-prompt
  component is a fixed per-generation charge (~1016–1266 tokens).
- `cache_creation_input_tokens` is always 0.
- Thinking tokens (#10) are excluded from the displayed `output_tokens` (#9),
  ride in `extra_total_tokens` (so they still count toward total tokens), and
  are folded into billable output tokens for cost calculation.
- Rows where input + output + cache-read + thinking are all 0 are skipped.
- The source stores no cost; cost comes from the shared pricing pipeline.

## Model aliases

Raw ids are mapped to LiteLLM-priced names case-insensitively; unknown ids
pass through unchanged:

```text
model_placeholder_m26 → claude-opus-4-6
model_placeholder_m35 → claude-sonnet-4-6
model_placeholder_m36/m37/m16 → gemini-3.1-pro
model_placeholder_m18/m84/m47 → gemini-3-flash-preview
model_placeholder_m132/m133 → gemini-3.5-flash-high
model_placeholder_m187 → gemini-3.5-flash-extra-low
model_placeholder_m20 → gemini-3.5-flash-medium
model_openai_gpt_oss_120b_medium → gpt-oss-120b-medium
gemini-pro-default, gemini-pro-agent → gemini-3.1-pro
gemini-3-flash-agent/-a/-b → gemini-3.5-flash-high
gemini-3-flash-c, gemini-3-flash → gemini-3-flash-preview
gemini-3.5-flash-low → gemini-3.5-flash-medium
gemini-3.1-pro-high/-low → gemini-3.1-pro
gemini-3-pro-high/-low → gemini-3-pro
claude-opus-4-6-thinking → claude-opus-4-6
claude-sonnet-4-6-thinking → claude-sonnet-4-6
```
