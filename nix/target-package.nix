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
      # NOTE(2026-10-28, #1984): clippy runs on the host layer, so a target layer
      # needs only build output; crane's default `cargo check` pass would compile
      # every dependency a second time for nothing.
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
    # NOTE(2026-06-11, #1279): CI pins these artifacts with a Nix profile root on
    # its sticky disk. Unreferenced once the binary is built, they would be
    # dropped by the disk's GC-root-respecting trim and every dependency rebuilt.
    passthru = { inherit cargoArtifacts dependencyArtifacts workspaceArtifacts; };
    meta = package.meta // {
      inherit description;
    };
  }
)
