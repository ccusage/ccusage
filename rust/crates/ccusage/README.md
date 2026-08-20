# ccusage

The distributed binary. It stays thin on purpose: argument parsing lives in
`ccusage-cli-parser`, shared runtime behavior in `ccusage-core`, and every data
source in its own `ccusage-adapter-*` crate.

## Owns

- `main.rs` — startup, command dispatch, and the version string the release
  embeds through `CCUSAGE_VERSION`.
- `cli.rs` — the parse entry point and the config context it passes to the parser.
- `commands/` — the command implementations that are not an agent report, such as
  `blocks` and `statusline`.
- `adapter/` — the thin aliases that map each `ccusage <agent>` subcommand to its
  adapter crate.
- `bin/generate_config_schema.rs` — writes `apps/ccusage/config-schema.json`; the
  `config-schema` flake check fails when the committed file drifts.

## Depends on

- `ccusage-adapter-all`
- `ccusage-adapter-amp`
- `ccusage-adapter-antigravity`
- `ccusage-adapter-claude`
- `ccusage-adapter-codebuff`
- `ccusage-adapter-codex`
- `ccusage-adapter-common`
- `ccusage-adapter-copilot`
- `ccusage-adapter-droid`
- `ccusage-adapter-gemini`
- `ccusage-adapter-goose`
- `ccusage-adapter-grok`
- `ccusage-adapter-hermes`
- `ccusage-adapter-kilo`
- `ccusage-adapter-kimi`
- `ccusage-adapter-openclaw`
- `ccusage-adapter-opencode`
- `ccusage-adapter-pi`
- `ccusage-adapter-qwen`
- `ccusage-cli`
- `ccusage-cli-parser`
- `ccusage-config`
- `ccusage-core`
- `serde`
- `serde_json`

## Build layer

Outside the Crane artifact layers: it is compiled with the final binary, so editing it leaves the cached layers untouched. Its link step is the dominant cost of a warm build, because the release
profile uses fat LTO with a single codegen unit.
