---
name: rust-binary-size
description: Guides Rust binary size reduction for ccusage. Use when changing the rust/Cargo.toml release profile, dependency features, native packaging size, or investigating executable bloat with cargo bloat.
paths:
  - 'rust/Cargo.toml'
  - 'rust/**/*.rs'
  - 'rust/**/*.toml'
  - 'apps/ccusage/scripts/**'
globs: 'rust/**/*.rs,rust/**/*.toml,apps/ccusage/scripts/**'
---

# Rust Binary Size

https://github.com/johnthagen/min-sized-rust

## Baseline

`[profile.release]` in `rust/Cargo.toml` already applies the `min-sized-rust`
settings, including a separate `opt-level` for dependencies. Read it before adding
anything, and change it only when a measurement argues for it.

What ends up in a published platform package is the single `bin/ccusage` staged by
the Nushell scripts in `apps/ccusage/scripts/`, so packaged size tracks the binary
almost exactly. The `development` skill covers that packaging seam.

## Investigation

Measure before editing code or dependencies:

```sh
direnv exec . cargo build --manifest-path rust/Cargo.toml --release --bin ccusage
ls -lh rust/target/release/ccusage
```

When the release profile does not explain a regression, look at feature flags
(`cargo tree -e features -p ccusage`) and large symbols (`cargo bloat --release
--bin ccusage --crates`), both against the same manifest. `cargo bloat` is not in
the dev shell; the `missing-tools` skill covers running it without a flake change.

## Change Ordering

Low-risk first: drop unnecessary dependency default features once tests show they
are unused, narrow optional features rather than swapping a well-fitting crate, and
remove dead release-only code paths or assets. CLI behavior, JSON output, table
output, and packaging semantics stay unchanged unless the user asks otherwise.

These are opt-in experiments, appropriate only when the user asks for an aggressive
minimum-size push: nightly-only flags (`-Zlocation-detail`, `-Zfmt-debug`,
`panic=immediate-abort`, `build-std`), `#![no_std]`/`#![no_main]` with manual stdio,
binary packers such as UPX, and `prefer-dynamic` linking.

## Validation

For release-profile or packaging changes, rebuild the native CLI, compare against
the earlier measurement, and record the command and result in the PR body or review
reply. Repo-wide format and test recipes are in the `development` skill.
