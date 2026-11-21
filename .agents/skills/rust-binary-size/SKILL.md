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

Review existing `[profile.release]` settings in `rust/Cargo.toml` before
changing the profile. Measure `bin/ccusage` for platform package size.

## Investigation

Measure before editing code or dependencies, with the shipped artifacts on
CI rather than a local build:

```sh
gh workflow run binary-size.yaml --repo ccusage/ccusage -f refs='main <branch>'
```

It builds pushed refs only (push the branch first) for every target in parallel and reports bytes, sections and
symbol growth. A local `cargo build --release` is only for a
quick check of one change.

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

For release-profile or packaging changes, re-run `binary-size.yaml` with `main`
and the branch, and record the run link and the size table in the PR body or
review reply. Repo-wide format and test recipes are in the `development` skill.
