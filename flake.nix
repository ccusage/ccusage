{
  description = "Usage analysis tool for Claude Code";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    # NOTE(2026-11-17): Tirith 0.4.2 rejects valid workflow bodies as incomplete (sheeki03/tirith#260). Keep the working pin until the workflow scan passes.
    tirith-nixpkgs.url = "github:NixOS/nixpkgs/91cc1fdf6831e29b6c98768e721a72241f3d0797";
    bun2nix = {
      url = "github:nix-community/bun2nix/0456acb1b7394fc14c414b056aa889df5124943a";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
    flake-parts.url = "github:hercules-ci/flake-parts";
    agent-skills = {
      url = "github:Kyure-A/agent-skills-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    git-hooks = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    litellm = {
      url = "file+https://raw.githubusercontent.com/BerriAI/litellm/89813239c4dadc032818ea0567bb13795ad5ee5c/model_prices_and_context_window.json";
      flake = false;
    };
    models-dev = {
      url = "github:anomalyco/models.dev";
      flake = false;
    };
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];

      imports = [
        inputs.treefmt-nix.flakeModule
        inputs.git-hooks.flakeModule
        ./nix/system.nix
        ./nix/agent-skills.nix
        ./nix/treefmt.nix
        ./nix/git-hooks.nix
        ./nix/packages.nix
        ./nix/pnpm.nix
        ./nix/tirith.nix
        ./nix/static-package.nix
        ./nix/darwin-package.nix
        ./nix/tests.nix
        ./nix/checks.nix
        ./nix/dev-shell.nix
      ];
    };
}
