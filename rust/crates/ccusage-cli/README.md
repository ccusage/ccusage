# ccusage-cli

Plain arguments shared by the binary and adapters. Use
`home::claude_config_dirs` for both Claude log discovery and ccusage configuration
discovery.

## Build layer

Keep this crate free of build scripts and embedded assets.
Core and every adapter build on it in the `foundation` Crane layer. Put parsing
and help generation in `ccusage-cli-parser`.
