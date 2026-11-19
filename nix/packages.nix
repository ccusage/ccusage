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
      pkgs,
      craneLib,
      rustToolchain,
      ...
    }:
    let
      ccusage = import ../default.nix {
        inherit
          craneLib
          inputs
          pkgs
          root
          ;
      };
      ccusageProgram = pkgs.lib.getExe' ccusage "ccusage";
      bunNodeModules = pkgs.callPackage ../nix/bun-node-modules.nix {
        bun2nix = inputs.bun2nix.packages.${system}.default;
      };
      bunCli = pkgs.callPackage ../nix/bun-cli.nix { inherit bunNodeModules; };
      changelogithub = pkgs.callPackage ../nix/tools/changelogithub { inherit bunCli; };
      models-dev-pricing = pkgs.callPackage ../nix/tools/models-dev-gen {
        inherit bunNodeModules;
        modelsDevSrc = inputs.models-dev;
      };
      cargo-hawk = pkgs.callPackage ./cargo-hawk.nix { inherit rustToolchain; };
      actionlint = pkgs.callPackage ./actionlint.nix { };
      oxlint = pkgs.callPackage ../nix/tools/oxlint { inherit bunCli; };
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
        inherit
          actionlint
          cargo-hawk
          ccusage
          changelogithub
          models-dev-pricing
          oxlint
          publint
          ;
      };
    };
}
