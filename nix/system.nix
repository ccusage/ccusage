{ inputs, ... }:
{
  perSystem =
    { config, system, ... }:
    let
      pkgs = import inputs.nixpkgs {
        inherit system;
        overlays = [ inputs.rust-overlay.overlays.default ];
      };
      rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml;
      craneLib = (inputs.crane.mkLib pkgs).overrideToolchain rustToolchain;
    in
    {
      _module.args = {
        inherit pkgs rustToolchain craneLib;
        testArtifacts = import ./test-artifacts.nix {
          inherit craneLib;
          cargoHawk = config.packages.cargo-hawk;
          inherit (config.packages.ccusage.passthru) depsOnlyArgs;
        };
      };
    };
}
