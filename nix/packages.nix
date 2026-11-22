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
      # NOTE(2026-06-11, #1260): On Linux this glibc-dynamic build has a runpath into /nix/store. Release binaries use the static build so they run without a Nix store.
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
      blacksmith = pkgs.callPackage ../nix/tools/blacksmith { };
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
          blacksmith
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
