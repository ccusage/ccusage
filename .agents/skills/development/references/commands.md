# Commands, Dependencies, Validation, Releases

## Commands

Use `just` as the project entry point.

Work inside the pinned Nix dev shell. Run `direnv allow` when initializing it;
prefix one-offs with `direnv exec .` from outside it.

Use `ccusage` with agent subcommands, such as `ccusage codex daily`, in code,
tests, and examples.

Choose Rust for production binaries and CPU- or memory-intensive tooling,
Nushell for small orchestration, Babashka for automation with a reusable data
model, substantial transformations, reporting, or its own tests, and TypeScript
for JavaScript APIs or work that benefits from the npm ecosystem.

## Adding A Dependency Or Tool

Runtime libraries for bundled packages go in `devDependencies` unless the user
asks otherwise.

`comma` and `nix run` are fine for one-off investigation, but anything used
repeatedly belongs in the repo: system and dev-shell CLIs in `flake.nix`
(`nix/dev-shell.nix` is the current list), JS/TS tooling and scripts in
`package.json`. Land the matching lockfile update — `flake.lock`,
`pnpm-lock.yaml` — in the same commit so the addition stays independently
revertable. Keep Cargo's `global-min-publish-age` in `.cargo/config.toml` aligned
with `minimumReleaseAge` in `pnpm-workspace.yaml`.

## Validation

Edit `nix/git-hooks.nix` to change Git hooks.

Let Git hooks and CI run the checks they enforce; do not repeat them manually
as a routine prerequisite to committing or pushing. Choose focused local checks
when they help develop a change or cover a gap in that automation. Prose-only
and license-only changes need no runtime tests or type checks.

## Releases

Releases are managed by tagpr: merging the auto-generated release PR tags the
merge commit and triggers the publish workflow. The bump is patch by default;
label a merged PR `minor` or `major` to raise it.

https://github.com/Songmu/tagpr

tagpr runs with an installation token of the release GitHub App, not
`GITHUB_TOKEN`, so the release PR gets the same CI and title checks as any
other; never dispatch CI or post commit statuses for it. The App is owned by
the `ccusage` organization, installed on this repository only, has no webhook,
and has repository permissions Contents read/write, Pull requests read/write,
Issues read, and nothing else. Its Client ID is the `RELEASE_APP_CLIENT_ID`
secret and its private key (the whole `.pem`) is `RELEASE_APP_PRIVATE_KEY`;
the tagpr job fails up front when either is missing.
