{
  bun2nix,
  lib,
  stdenvNoCC,
}:
{
  toolDir,
}:
let
  manifest = lib.importJSON (toolDir + "/package.json");
in
stdenvNoCC.mkDerivation {
  pname = "${manifest.name}-node-modules";
  inherit (manifest) version;

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

  bunInstallFlags = "--linker=isolated --backend=copyfile --frozen-lockfile";

  dontRunLifecycleScripts = true;
  dontUseBunBuild = true;
  dontUseBunCheck = true;
  dontUseBunInstall = true;

  installPhase = ''
    runHook preInstall

    mkdir -p "$out"
    cp -R node_modules "$out/node_modules"
    chmod -R u+w "$out/node_modules"

    runHook postInstall
  '';
}
