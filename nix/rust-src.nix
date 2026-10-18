# Source subset the Rust build, tests, and clippy actually read at compile time,
# so those derivations only rebuild when something they depend on changes — not
# on every docs, TypeScript, or workflow edit elsewhere in the repo:
#   * ccusage-core/build.rs   reads ../../../flake.lock
#   * config_schema.rs tests  include_str! ../../../../ccusage.example.json
#   * main.rs tests           include_str! ../../../../package.json
{ lib, root }:
lib.fileset.toSource {
  inherit root;
  fileset = lib.fileset.unions [
    (root + /rust)
    (root + /flake.lock)
    (root + /ccusage.example.json)
    (root + /package.json)
  ];
}
