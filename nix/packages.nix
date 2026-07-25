{
  inputs,
  ...
}:
let
  root = ./..;
in
{
  perSystem =
    {
      system,
      ...
    }:
    let
      pkgs = import inputs.nixpkgs {
        inherit system;
        overlays = [ inputs.rust-overlay.overlays.default ];
      };
      rustToolchain = pkgs.rust-bin.fromRustupToolchainFile (root + /rust-toolchain.toml);
      craneLib = (inputs.crane.mkLib pkgs).overrideToolchain rustToolchain;
      ccusage = import ../default.nix {
        inherit
          craneLib
          inputs
          pkgs
          root
          ;
      };
      ccusageProgram = pkgs.lib.getExe' ccusage "ccusage";
      # Shared builders for the `nix/tools/*` dependency manifests; see
      # nix/bun-node-modules.nix and nix/bun-cli.nix.
      bunNodeModules = pkgs.callPackage ../nix/bun-node-modules.nix {
        bun2nix = inputs.bun2nix.packages.${system}.default;
      };
      bunCli = pkgs.callPackage ../nix/bun-cli.nix { inherit bunNodeModules; };
      # Regeneration-only output for committed models.dev snapshots;
      # `just gen-models-dev-pricing` builds this and copies them into the source
      # tree. It is not part of the ccusage build, which embeds the committed files.
      models-dev-pricing = pkgs.callPackage ../nix/tools/models-dev-gen {
        inherit bunNodeModules;
        modelsDevSrc = inputs.models-dev;
      };
      publint = pkgs.callPackage ../nix/tools/publint { inherit bunCli; };
    in
    {
      apps = {
        default = {
          type = "app";
          program = ccusageProgram;
        };
        ccusage = {
          type = "app";
          program = ccusageProgram;
        };
      };

      packages = {
        default = ccusage;
        inherit ccusage models-dev-pricing publint;
      };
    };
}
