# Nix-built `publint` CLI used by the package checks (`nix flake check`) and by
# the `build`/`prepack` scripts of every published package.
#
# The dependency set sits next to this file: `bun.lock` and `bun.nix` are real
# files that `bun install` and `bun2nix` regenerate, so bumping publint is
# `just gen-bun-nix` instead of hand-editing a lockfile embedded in a Nix string.
{ bunCli, lib }:
bunCli {
  toolDir = ./.;
  pname = "publint";
  meta = {
    description = "Lint packaging errors";
    homepage = "https://publint.dev";
    license = lib.licenses.mit;
  };
}
