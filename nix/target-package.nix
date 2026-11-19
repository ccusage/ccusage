{
  package,
  craneLib,
  pkgs,
  root,
  target,
  overrides ? { },
  doInstallCheck,
  postInstall,
  description,
}:
let
  cargoExtraArgs = "-p ccusage --bin ccusage --target ${target}";
  commonArgs = package.passthru.commonArgs // overrides // { inherit cargoExtraArgs; };
  depsOnlyArgs =
    package.passthru.depsOnlyArgs
    // overrides
    // {
      inherit cargoExtraArgs;
      buildPhaseCargoCommand = "cargoWithProfile build ${cargoExtraArgs}";
    };
  dependencyArtifacts = craneLib.buildDepsOnly depsOnlyArgs;
  workspaceArtifacts = import ./cargo-artifacts.nix {
    inherit
      root
      pkgs
      craneLib
      commonArgs
      ;
    inherit (pkgs) lib;
    inherit (depsOnlyArgs) dummySrc;
    cargoArtifacts = dependencyArtifacts;
  };
  cargoArtifacts = workspaceArtifacts.adapters;
in
craneLib.buildPackage (
  commonArgs
  // {
    inherit cargoArtifacts doInstallCheck postInstall;
    passthru = { inherit cargoArtifacts dependencyArtifacts workspaceArtifacts; };
    meta = package.meta // {
      inherit description;
    };
  }
)
