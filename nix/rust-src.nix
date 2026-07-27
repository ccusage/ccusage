# Source subset the Rust build, tests, and clippy actually read at compile time,
# so those derivations only rebuild when something they depend on changes — not
# on every docs, TypeScript, or workflow edit elsewhere in the repo:
#   * ccusage-core/build.rs   reads ../../../flake.lock
#   * config_schema.rs tests  include_str! ../../../../ccusage.example.json
#   * main.rs tests           include_str! ../../../../package.json
{ nixFilter, root }:
nixFilter {
  inherit root;
  include = [
    "rust"
    "flake.lock"
    "ccusage.example.json"
    "package.json"
  ];
}
