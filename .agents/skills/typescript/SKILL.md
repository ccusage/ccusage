---
name: typescript
description: Guides ccusage TypeScript and JavaScript work. Use before reading or editing .ts, .tsx, .js, or .jsx files, including the npm launcher, Node tests, build and fixture scripts, mocks, and typed literals.
paths:
  - '**/*.ts'
  - '**/*.tsx'
  - '**/*.js'
  - '**/*.jsx'
globs: '*.ts,*.tsx,*.js,*.jsx'
---

# ccusage TypeScript

Keep adapter runtime behavior in Rust. Use local `.ts` extensions, static
imports, Node path utilities, and exports with outside consumers.

Validate literals and mocks with `satisfies`. Reserve assertions for untyped
boundaries and nominal APIs. Use `as const` where exact literal values matter
and `satisfies never` for exhaustive switches. Give `@ts-expect-error` a dated
reason and avoid `@ts-ignore`.

Run `just test-node` and `just typecheck`. For performance changes, follow
`profile`. For structural searches, use `ast-grep`.
