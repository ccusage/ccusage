# ccusage-config

`ccusage.json` loading, validation, and the JSON Schema published for editors.

## Owns

- `config.rs`
- `config_schema.rs`

Only the binary reads configuration, and the schema derives pull `schemars` in,
so keeping both here means neither reaches the critical path of `ccusage-core` or
any adapter. `ccusage-cli-parser` uses it as a dev-dependency, for the tests that
assert how a parsed command line and a config file combine.

## Public surface

- `config::ConfigContext`
- `config_schema::generate_config_schema_json`

## Depends on

- `ccusage-cli`
- `ccusage-core`
- `schemars`
- `serde`
- `serde_json`

## Build layer

Outside the Crane artifact layers: it is compiled with the final binary, so editing it leaves the cached layers untouched.
