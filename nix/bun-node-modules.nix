# Turn one of the `nix/tools/*` dependency manifests into a `node_modules` tree.
#
# Each tool directory holds a `package.json`, the `bun.lock` that `bun install`
# resolved from it, and the `bun.nix` that `bun2nix` derived from that lockfile
# (regenerate both with `just gen-bun-nix`). bun2nix fetches every tarball as a
# fixed-output derivation, so the install below needs no network access.
{
  bun2nix,
  lib,
  stdenvNoCC,
}:
{
  # Directory containing package.json, bun.lock and bun.nix.
  toolDir,
}:
let
  manifest = lib.importJSON (toolDir + "/package.json");
in
stdenvNoCC.mkDerivation {
  pname = "${manifest.name}-node-modules";
  inherit (manifest) version;

  # Only the two files `bun install` reads. A tool directory also holds its Nix
  # expression and any scripts the tool runs, and none of those should force the
  # dependency tree to rebuild. (`bun.nix` is read at evaluation time, so it does
  # not belong in the build inputs either.)
  src = lib.fileset.toSource {
    root = toolDir;
    fileset = lib.fileset.unions [
      (toolDir + "/package.json")
      (toolDir + "/bun.lock")
    ];
  };

  nativeBuildInputs = [ bun2nix.hook ];

  bunDeps = bun2nix.fetchBunDeps {
    bunNix = toolDir + "/bun.nix";
  };

  # The hook only honors a wholesale replacement of its default flags, so the
  # linker and backend are restated here alongside `--frozen-lockfile`.
  #
  # `--frozen-lockfile`: fail loudly if bun wants to resolve anything the
  # committed lockfile does not already pin, instead of reaching for the network.
  # `--linker=isolated`: what the hook defaults to on every platform.
  # `--backend=copyfile`: one backend on every platform. The hook picks
  # `symlink` on Darwin and bun's default elsewhere, which suits its own
  # `bun build --compile` flow where `node_modules` is transient; here the tree
  # *is* the output, so it should not depend on backend semantics.
  bunInstallFlags = "--linker=isolated --backend=copyfile --frozen-lockfile";

  # These manifests pin plain library packages, so there are no lifecycle
  # scripts to run and nothing for bun to build, check or install.
  dontRunLifecycleScripts = true;
  dontUseBunBuild = true;
  dontUseBunCheck = true;
  dontUseBunInstall = true;

  installPhase = ''
    runHook preInstall

    mkdir -p "$out"
    cp -R node_modules "$out/node_modules"
    # The staged dependencies come out of the store read-only and `cp -R` keeps
    # those modes; consumers copy this tree into a writable build directory.
    chmod -R u+w "$out/node_modules"

    runHook postInstall
  '';
}
