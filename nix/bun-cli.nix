# Wrap a CLI published on npm as a Nix package, from one of the `nix/tools/*`
# dependency manifests.
#
# The manifest pins the CLI (and nothing else) as a dependency, so `bun.lock`
# and `bun.nix` next to it describe the whole closure; `just gen-bun-nix`
# regenerates them.
{
  bunNodeModules,
  lib,
  makeWrapper,
  nodejs,
  stdenvNoCC,
}:
{
  # Directory containing package.json, bun.lock and bun.nix.
  toolDir,
  # npm package name; also the name of the wrapper this produces.
  pname,
  # Extra packages the CLI shells out to at runtime.
  runtimeInputs ? [ ],
  meta ? { },
}:
let
  manifest = lib.importJSON (toolDir + "/package.json");
  nodeModules = bunNodeModules { inherit toolDir; };
in
stdenvNoCC.mkDerivation {
  inherit pname;
  inherit (manifest) version;

  dontUnpack = true;

  nativeBuildInputs = [ makeWrapper ];

  # The shebangs rewritten below are already absolute store paths, so the
  # default fixupPhase would leave them alone; skipping it keeps the intent
  # explicit and the phase from walking the whole dependency tree again.
  dontPatchShebangs = true;

  installPhase = ''
    runHook preInstall

    toolRoot="$out/lib/${pname}"
    mkdir -p "$toolRoot" "$out/bin"
    cp -R ${nodeModules}/node_modules "$toolRoot/node_modules"
    chmod -R u+w "$toolRoot/node_modules"

    # bun2nix stages each dependency through its own derivation and patches
    # shebangs there against bun's `bun-with-fake-node` shim, so CLIs arrive
    # already pointing at Bun's Node emulation. Repoint them at real Node:
    # these are Node programs, and the baked-in path would otherwise keep bun
    # in the runtime closure.
    grep -rlE '^#!.*bun-with-fake-node/bin/node' "$toolRoot" | while IFS= read -r script; do
      sed -i "1s|^#!.*bun-with-fake-node/bin/node|#!${lib.getExe nodejs}|" "$script"
    done

    # Invoke Node directly rather than relying on the shebang, so the wrapper
    # keeps working whatever bun2nix decides to write there.
    makeWrapper ${lib.getExe nodejs} "$out/bin/${pname}" \
      --add-flags "$toolRoot/node_modules/.bin/${pname}" \
      ${lib.optionalString (runtimeInputs != [ ]) ''
        --prefix PATH : ${lib.makeBinPath runtimeInputs}
      ''}

    runHook postInstall
  '';

  meta = {
    mainProgram = pname;
    platforms = lib.platforms.all;
  }
  // meta;
}
