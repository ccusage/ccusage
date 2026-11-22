{
  craneLib,
  inputs,
  pkgs,
  root ? ./.,
  telemetryEndpoint ? null,
}:
pkgs.callPackage ./package.nix {
  inherit
    craneLib
    inputs
    pkgs
    root
    telemetryEndpoint
    ;
}
