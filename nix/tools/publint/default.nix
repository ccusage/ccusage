# Nix-built `publint` CLI used by the package checks (`nix flake check`) and by
# the `build`/`prepack` scripts of every published package.
#
# The dependency set sits next to this file: `bun.lock` and `bun.nix` are real
# files that `bun install` and `bun2nix` regenerate, so bumping publint is
# `just gen-bun-nix` instead of hand-editing a lockfile embedded in a Nix string.
{
  bunNodeModules,
  lib,
  makeWrapper,
  nodejs,
  stdenvNoCC,
}:
let
  manifest = lib.importJSON ./package.json;
  nodeModules = bunNodeModules { toolDir = ./.; };
in
stdenvNoCC.mkDerivation {
  pname = "publint";
  inherit (manifest) version;

  dontUnpack = true;

  nativeBuildInputs = [ makeWrapper ];

  installPhase = ''
    runHook preInstall

    toolRoot="$out/lib/publint"
    mkdir -p "$toolRoot" "$out/bin"
    cp -R ${nodeModules}/node_modules "$toolRoot/node_modules"
    chmod -R u+w "$toolRoot/node_modules"

    # bun2nix stages each dependency through its own derivation and patches
    # shebangs there against bun's `bun-with-fake-node` shim, so the CLI arrives
    # already pointing at Bun's Node emulation. Repoint it at real Node: the
    # baked-in path would otherwise keep bun in this package's runtime closure,
    # and publint is a Node packaging linter, not a Bun program.
    grep -rlE '^#!.*bun-with-fake-node/bin/node' "$toolRoot" | while IFS= read -r script; do
      sed -i "1s|^#!.*bun-with-fake-node/bin/node|#!${lib.getExe nodejs}|" "$script"
    done

    makeWrapper ${lib.getExe nodejs} "$out/bin/publint" \
      --add-flags "$toolRoot/node_modules/.bin/publint"

    runHook postInstall
  '';

  meta = {
    description = "Lint packaging errors";
    homepage = "https://publint.dev";
    license = lib.licenses.mit;
    mainProgram = "publint";
    platforms = lib.platforms.all;
  };
}
