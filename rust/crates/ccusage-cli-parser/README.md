# ccusage-cli-parser

Argument parsing and `--help` rendering: it turns a command line, plus any
`ccusage.json` defaults, into the `Cli` value the binary runs.

## Owns

- `parser.rs` — the hand-written parser, including config-file merging through
  the `CliConfig` trait.
- `arg_parser.rs` — the token-level argument reader.
- `help.rs`, `help_codegen.rs`, `build.rs` — help text generation from
  `src/cli-help.json` and `src/cli-commands.json`.
- `tests.rs` — parse-shape and help snapshot tests, plus the config-merge tests.

Only the `ccusage` binary depends on this crate. The argument types themselves
stay in `ccusage-cli`, so editing help text does not reach `ccusage-core` or any
adapter.

## Depends on

- `ccusage-cli`

## Build layer

Outside the Crane artifact layers: it is compiled with the final binary, so editing it leaves the cached layers untouched. Verified by comparing the adapters layer derivation path before and
after editing `src/cli-help.json`.
