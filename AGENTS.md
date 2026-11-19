# AGENTS.md

Routing plus the policies that are true before any task starts. Task-time detail lives in the
skills below and loads when the task triggers them.

## Skill Routing

- `development` - monorepo layout, `just` recipes, dev-shell environment, dependency and tooling policy, validation, releases.
- `rust` - `rust/crates` and `rust/adapters` implementation: crate roles, module layout, visibility and `just hawk`, embedded pricing, behavior parity.
- `typescript` - `.ts`, `.tsx`, `.js`, and `.jsx` package and tooling code, typed literals, launchers, schema tooling.
- `agent-sources` - agent log locations, raw record shapes, token mappings, cost modes, source-specific CLI behavior.
- `testing` - cargo and Node tests, CLI snapshots, fixtures, pricing and model tests.
- `tdd` - Red-Green-Refactor workflow for logic changes.
- `profile` - CLI and script performance, branch-vs-main comparisons.
- `rust-binary-size` - release profile, dependency features, native packaging size, executable bloat.
- `reduce-similarities` - duplicate Rust code detection with `similarity-rs`.
- `ast-grep` - structural searches in Rust or TypeScript.
- `nushell` - `.nu` scripts, Nix shebangs, and the recipes and workflows that call them.
- `cmux-debug` - output that depends on real terminal geometry: responsive tables, progress, spinners.
- `docs` - whether and where a change needs documentation updates; run it whenever a user-facing agent, command, option, report mode, config key, JSON field, or screenshot changes.
- `skill-creator` - authoring or editing `.agents/skills`.
- `commit` - atomic Conventional Commits and patch-based staging.
- `create-pr` - branch through merged PR: pushing, AI review requests, review-thread replies, CI.
- `fix-ci` - failing GitHub Actions checks.

## Repository Shape

The production CLI is Rust-first. Source-specific runtime behavior belongs in
`rust/adapters/<agent>`, shared behavior in `ccusage-core` or `ccusage-adapter-common`.
`apps/ccusage` is the npm surface only - bin launcher, schema artifact, packaging and benchmark
scripts - and not a place for adapter logic.

Read the nearest `AGENTS.md` before editing:

- `apps/ccusage/AGENTS.md` - published npm package
- `rust/adapters/AGENTS.md` - adapter crates
- `docs/AGENTS.md` - VitePress documentation site

## Policy

- The canonical command is `ccusage` with agent subcommands, such as `ccusage codex daily`. The
  standalone wrapper packages `ccusage-amp`, `ccusage-codex`, `ccusage-opencode`, and `ccusage-pi`
  were removed; do not reintroduce them in code, docs, tests, or examples.
- Runtime libraries for bundled packages go in `devDependencies` unless the user asks otherwise.
- Language choice: Rust for production binaries and CPU- or memory-intensive tooling; Nushell for
  small automation that mostly orchestrates external commands, files, HTTP, and JSON; Babashka once
  automation grows a reusable data model, several transformation stages, substantial reporting
  logic, or its own unit tests; TypeScript when the work benefits from the npm and JavaScript
  ecosystem or exposes a JavaScript API.
- Repository-facing GitHub communication - issue comments, PR descriptions, review replies, triage
  notes, bot-directed replies - is US English.
- PR branches are squash-merged, so prefer small stacked follow-up commits over
  `git commit --amend` unless the user asks for an amend.
- Work happens inside the pinned Nix dev shell: run `direnv allow` when first initializing this
  environment, and prefix one-offs with `direnv exec .` from outside it.
