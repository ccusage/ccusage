# AGENTS.md

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
- `code-comments` - writing, reviewing, or deleting code comments in any repository language.
- `nushell` - `.nu` scripts, Nix shebangs, and the recipes and workflows that call them.
- `cmux-debug` - output that depends on real terminal geometry: responsive tables, progress, spinners.
- `docs` - whether and where a change needs documentation updates; run it whenever a user-facing agent, command, option, report mode, config key, JSON field, or screenshot changes.
- `skill-creator` - authoring or editing `.agents/skills`.
- `commit` - atomic Conventional Commits and patch-based staging.
- `create-pr` - branch through merged PR: pushing, AI review requests, review-thread replies, CI.
- `fix-ci` - failing GitHub Actions checks.

## Repository Shape

The production CLI is Rust-first. Put source-specific runtime behavior in
`rust/adapters/<agent>` and shared behavior in `ccusage-core` or `ccusage-adapter-common`.
Keep adapter logic out of the npm package in `apps/ccusage`.

Read the nearest `AGENTS.md` before editing:

- `apps/ccusage/AGENTS.md` - published npm package
- `rust/adapters/AGENTS.md` - adapter crates
- `docs/AGENTS.md` - VitePress documentation site

## Policy

- Use `ccusage` with agent subcommands, such as `ccusage codex daily`, in code, docs, tests, and examples. Keep the retired `ccusage-amp`, `ccusage-codex`, `ccusage-opencode`, and `ccusage-pi` wrapper packages out of the workspace.
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

## Pull Requests

- Keep the diff on the task: no unrelated clean-up or refactoring.
- A bug fix removes the root cause, not the symptom, and starts with a regression test that fails on `main` for the reported reason (`tdd`).
- Report output and existing snapshots stay unchanged; when one must move, the PR body names the rows and why.
- The PR body lists the checks run; for a bug fix it also states the root cause and names the regression test.
