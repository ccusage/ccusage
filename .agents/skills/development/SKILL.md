---
name: development
description: Guides ccusage monorepo development. Use when editing workspace packages, the npm launcher or native packaging, dependencies, or shared configuration, and when running the `just` build, typecheck, test, format, or check recipes.
---

# ccusage Development

Make npm launcher and native staging changes in `apps/ccusage`. Update
`apps/ccusage/scripts/stage-native-package.nu` and
`apps/ccusage/scripts/verify-native-package.nu` together when changing the native
package layout.

Use `LOG_LEVEL=0` for quiet captured output and `LOG_LEVEL=4` for pricing refresh
detail.

Read [commands](references/commands.md) for `just`, dependency changes, validation,
and releases.
