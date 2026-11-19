{ lib, ... }:
let
  root = ./..;
in
{
  perSystem =
    {
      config,
      pkgs,
      craneLib,
      testArtifacts,
      ...
    }:
    let
      inherit (config.packages.ccusage.passthru) commonArgs;
      testSrc = import ./rust-src.nix { inherit lib root; };
    in
    {
      packages.ccusage-tests = craneLib.cargoTest (
        commonArgs
        // testArtifacts.consumerArgs
        // {
          src = testSrc;
          sourceRoot = "source/rust";
          cargoLock = root + /rust/Cargo.lock;
          nativeBuildInputs = commonArgs.nativeBuildInputs or [ ] ++ [ pkgs.jq ];
          inherit (testArtifacts) cargoArtifacts;
          passthru = { inherit (testArtifacts) cargoArtifacts; };
          doCheck = true;
          cargoExtraArgs = "--workspace";
          TZDIR = "${pkgs.tzdata}/share/zoneinfo";
          preBuild = ''
            export HOME=$(mktemp -d)
          '';
        }
      );
    };
}
