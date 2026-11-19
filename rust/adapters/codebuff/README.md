# ccusage-adapter-codebuff

The Codebuff adapter: it turns per-channel `chat-messages.json` files
into the usage entries the reports render.

## Data source

- `${CODEBUFF_DATA_DIR:-~/.config/<channel>}/projects/**/chat-messages.json`
- Channels: `manicode`, `manicode-dev`, `manicode-staging`.

Usage records use their message timestamp or the timestamp in the chat directory
name. Records with neither are skipped with an invalid-timestamp diagnostic; the
chat file's modification time is not used, because it records the last save of
the whole chat, not when a message was sent.

## Build layer

Built in the `adapters` Crane artifact layer.
