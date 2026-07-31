---
name: development
description: Guides ccusage monorepo development. Use when editing workspace packages, the npm launcher or native packaging, dependencies, or shared configuration, and when running the `just` build, typecheck, test, format, or check recipes.
---

# ccusage Development

Root `AGENTS.md` holds the repository shape and the standing policies. This
skill is about working in the tree: the npm packaging seam, the recipes, and
validation.

## The npm Packaging Seam

`apps/ccusage` is the npm shell around the Rust binary — package metadata,
`config-schema.json`, and the packaging and benchmark scripts. Its
`src/cli.js` launcher resolves the platform binary from the matching
`@ccusage/ccusage-<platform>-<arch>` optional dependency and spawns it; those
packages live in `packages/ccusage-<platform>-<arch>` and ship nothing but
`bin/ccusage`.

The Nushell scripts in `apps/ccusage/scripts/` own the binary side and share
`native-binary.nu`: `ensure-native-binary.nu` puts a usable, portable binary in
place for a local build, `stage-native-package.nu` fills one platform package,
and `verify-native-package.nu` runs from each platform package's `prepack`. A
packaging change usually touches the launcher and those scripts together.

## Gotchas

- `.claude/skills` is generated from `.agents/skills` by `nix/agent-skills.nix`.
  Edit the source tree; leave the generated one uncommitted.
- `LOG_LEVEL` gates runtime noise (`rust/crates/ccusage-core/src/logger.rs`):
  `0` suppresses progress and box titles, `>= 4` logs pricing refresh detail.
  Use `LOG_LEVEL=0` whenever output is captured or compared.

`references/commands.md` covers `just`, where a new dependency or tool belongs,
validation, and releases.
