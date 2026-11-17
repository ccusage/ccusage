# ccusage-adapter-codebuff

The Codebuff adapter: it turns per-channel `chat-messages.json` files
into the usage entries the reports render.

## Owns

- `loader.rs` — reading the source, dedupe, and date filtering.
- `parser.rs` — raw record parsing, token mapping, and model naming.
- `paths.rs` — environment variables, default directories, and file discovery.

Anything that is not specific to this source belongs in `ccusage-core` or
`ccusage-adapter-common` instead.

## Data source

- `${CODEBUFF_DATA_DIR:-~/.config/<channel>}/projects/**/chat-messages.json`
- Channels: `manicode`, `manicode-dev`, `manicode-staging`.

Reads plain files through `ccusage-adapter-common`, which handles walking, size-balanced
chunking, and ordered parallel reads.

Usage records use their message timestamp or the timestamp in the chat directory
name. Records with neither are skipped with an invalid-timestamp diagnostic; the
chat file's modification time is not used, because it records the last save of
the whole chat, not when a message was sent.

## Public surface

- `SOURCE`
- `run`

## Depends on

- `ccusage-adapter-common`
- `ccusage-core`
- `jiff`
- `serde_json`

## Build layer

Built in the `adapters` Crane artifact layer; the layer compiles all adapters in one Cargo invocation, so they build concurrently.
