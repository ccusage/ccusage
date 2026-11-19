# Commands, Dependencies, Validation, Releases

## Commands

Use `just` as the project entry point. Run `just fmt` before read-only checks.

## Adding A Dependency Or Tool

`comma` and `nix run` are fine for one-off investigation, but anything used
repeatedly belongs in the repo: system and dev-shell CLIs in `flake.nix`
(`nix/dev-shell.nix` is the current list), JS/TS tooling and scripts in
`package.json`. Land the matching lockfile update — `flake.lock`,
`pnpm-lock.yaml` — in the same commit so the addition stays independently
revertable. Keep Cargo's `global-min-publish-age` in `.cargo/config.toml` aligned
with `minimumReleaseAge` in `pnpm-workspace.yaml`.

## Validation

Edit `nix/git-hooks.nix` to change Git hooks.

Run `just typecheck` and `just test` yourself when the change touches behavior,
types, or package code, or when the hooks and CI do not cover the edited files.
Narrower package recipes are useful while iterating; finish with the root ones.

## Releases

Releases are managed by tagpr: merging the auto-generated release PR tags the
merge commit and triggers the publish workflow. The bump is patch by default;
label a merged PR `minor` or `major` to raise it.

https://github.com/Songmu/tagpr
