# AGENTS.md

This file points agents at the right repo-local skills and keeps only the guidance that should always be visible.

## Skill Routing

Use these skills before working in this repository:

- `development` - monorepo layout, bundled CLI packaging, commands, code style, dependency policy, and post-change checks.
- `rust` - native Rust CLI implementation, parser/module layout, pricing embedding, and TypeScript parity checks.
- `profile` - Rust and TypeScript performance profiling, branch-vs-main speed comparisons, profile reading, and optimization validation.
- `testing` - Rust cargo tests, Node tests, CLI snapshots, Claude model names, LiteLLM pricing tests, and filesystem fixtures.
- `typescript` - TypeScript package/tooling work, package launchers, schema tooling, and typed fixtures.
- `agent-sources` - agent adapter log locations, token mappings, cost rules, and CLI behavior.
- `docs` - cross-repository documentation impact checks for README files, docs guides, VitePress navigation, screenshots, schema docs, and user-facing commands/options.
- `skill-creator` - repo-local skill creation, SKILL.md frontmatter, description trigger quality, and reference layout.
- `nushell` - Nushell scripts, Nix shebangs, GitHub Actions script calls, `nu-check`, and `nufmt` formatting.
- `ast-grep` - structural code searches in Rust or TypeScript and AST-based migration verification with the dev-shell `ast-grep` CLI.
- `rust-binary-size` - Rust release profile, dependency feature, native packaging size, and executable bloat reduction guidance based on min-sized-rust.
- `tdd` - Red-Green-Refactor workflow for logic changes.
- `reduce-similarities` - AST-based duplicate Rust code detection with similarity-rs; TypeScript duplication checks use `typescript` and `ast-grep`.
- `cmux-debug` - terminal UI and responsive table verification in cmux.
- `create-pr` - single entry point for PR work, from branch creation through AI review requests, review-thread replies, and passing CI.
- `fix-ci` - diagnose and fix failing GitHub Actions checks with `gh`, then push small follow-up commits.

## Monorepo Packages

Check the nearest package-specific `AGENTS.md` before editing package code:

- `apps/ccusage/AGENTS.md` - main Claude Code usage CLI and library
- `docs/AGENTS.md` - VitePress documentation site

## Always-On Reminders

- The canonical user-facing CLI is `ccusage` with agent subcommands such as `ccusage amp`, `ccusage codex`, `ccusage opencode`, and `ccusage pi`.
- Use Nushell for small structured automation that primarily orchestrates external commands, files, HTTP, and JSON.
- Use Babashka when automation develops a reusable data model, multiple transformation stages, substantial reporting logic, or focused unit tests.
- Use Rust for production binaries and CPU- or memory-intensive tooling.
- Use TypeScript when the implementation directly benefits from the npm and JavaScript ecosystem or exposes a JavaScript API.
- Standalone agent wrapper packages have been removed. Do not add docs, tests, or features that promote `ccusage-amp`, `ccusage-codex`, `ccusage-opencode`, or `ccusage-pi`.
- Runtime libraries for bundled packages belong in `devDependencies` unless explicitly requested otherwise.
- When initializing this repository environment, run `direnv allow` so the pinned Nix dev shell is activated.
- Prefer tools provided by the Nix dev shell before falling back to ad hoc installs: `rg`, `fd`, `fzf`, `delta`, `dust`, `jq`, `gh`, `hyperfine`, `similarity`, `ast-grep`, `typos`, and `typos-lsp`. When a missing tool would be useful for repeated agent work in this repository, add it to `flake.nix`.
- The production CLI is Rust-first under `rust/crates/ccusage`. Put new runtime behavior there unless the work is specifically about npm packaging, generated schemas, docs tooling, or benchmark scripts.
- For Rust code, keep modules small, keep `pub(crate)` surfaces narrow, prefer fixture-backed parser/loader tests, and run cargo checks through the `just` recipes when possible.
- TypeScript rules still apply to `.ts`, `.tsx`, `.js`, and `.jsx` package/tooling files. Use `typescript` there, especially `satisfies` and `as const satisfies` for typed literals.
- For TypeScript package code, use `logger.ts` instead of `console.log`, use `.ts` extensions for local imports, and avoid dynamic imports.
- After code changes, run `just fmt` when formatting may apply. Rely on git hooks and CI for covered checks; run `just typecheck` or `just test` manually when the change touches behavior, types, package code, or when hooks/CI do not cover the edited files. `just` is the single entry point for repo tasks (`just --list`); recipes route to the pnpm workspace or the Nix flake.
- PR branches are squash-merged by default; prefer stacked, small, revertable follow-up commits over `git commit --amend` unless explicitly requested.
- Use US English for repository-facing GitHub communication, including issue comments, PR descriptions, review replies, triage notes, and bot-directed replies.
- Do what has been asked, nothing more. Do not proactively create documentation files unless explicitly requested.

## Cross-Cutting Flow

For changes that affect user-facing agents, commands, options, report modes,
configuration, JSON output, screenshots, or examples:

1. Implement the runtime/package/docs change in the owning location.
2. Use the `docs` skill to audit documentation impact.
3. Update the root `README.md`, `apps/ccusage/README.md`, relevant `docs/guide/`
   pages, related cross-links, and VitePress navigation when the user-facing
   surface changed.
4. Skip documentation edits for internal-only refactors, test-only changes, or
   skill maintenance unless they change user-facing behavior.
