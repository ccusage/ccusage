# ccusage-cli-parser

Argument parsing and `--help` generation. Edit `src/cli-help.json` and
`src/cli-commands.json` for help text.

## Build layer

Keep this crate in the final binary layer and plain argument types in
`ccusage-cli`. Editing help leaves cached foundation and adapter layers intact.
