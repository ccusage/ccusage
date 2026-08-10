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

Runtime CLI behavior lives in Rust under `rust/`. New adapter logic belongs there,
not here, unless the user scopes work to the package layer. What remains in
TypeScript and JavaScript:

- `apps/ccusage/src/cli.js` — native binary launcher, covered by `cli.test.ts` beside it.
- `apps/ccusage/scripts/generate-large-fixture.ts` — benchmark fixture generator, run through `just generate-large-fixture`; it executes under Bun via a Nix shebang, and apart from its `bun-globals.d.ts` the rest of that directory is Nushell and Babashka.
- `nix/tools/models-dev-gen/` — models.dev pricing snapshot generator; `just gen-models-dev-pricing` reruns it, `just gen-bun-nix` refreshes its `bun.nix`.
- `docs/.vitepress/config.ts` and root TypeScript configuration or scripts, when the change is not docs-content-only.

## Style

Beyond ordinary TypeScript practice: `.ts` extensions on local imports, static
imports over dynamic ones, Node path utilities for file paths, and exports limited
to values used outside the module.

Type literals with `satisfies` rather than an `as` assertion, so excess and missing
properties still fail typecheck — object literals, mocks, config objects, fixture
data, expected rows. `as any` on a mock context loses that check entirely. Reach
for `as` only where `satisfies` cannot express the operation, such as narrowing
data from an external untyped boundary or adapting to an API that requires a
nominal type, and keep it local.

Add `as const` for static literal data where exact literal values or readonly
tuples help catch mistakes, including table-driven cases:

```ts
const reportCases = [
	{ type: 'daily', period: '2026-05-16' },
	{ type: 'monthly', period: '2026-05' },
] as const satisfies readonly ReportCase[];
```

Close switches over discriminated or literal unions with `satisfies never` in the
default branch, so a new variant fails typecheck until its branch exists. Suppress
with `@ts-expect-error` plus a short explanation, not `@ts-ignore`, so the
suppression fails once the underlying error disappears.

## Routing

- Node tests: `just test-node` during iteration; layout and runner wiring in `.agents/skills/testing/references/node-test.md`.
- Launcher, benchmark, packaging script, or native CLI performance: `profile`.
- TypeScript duplication checks: `ast-grep` or `rg`. There is no similarity-ts workflow here.
- Repo-wide format, typecheck, and check recipes: `development`.
