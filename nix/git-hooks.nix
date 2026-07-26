{
  inputs,
  lib,
  ...
}:
let
  root = ./..;
in
{
  perSystem =
    {
      config,
      system,
      ...
    }:
    let
      pkgs = import inputs.nixpkgs {
        inherit system;
        overlays = [ inputs.rust-overlay.overlays.default ];
      };
      rustToolchain = pkgs.rust-bin.fromRustupToolchainFile (root + /rust-toolchain.toml);
    in
    {
      pre-commit = {
        check.enable = false;
        inherit pkgs;
        settings = {
          src = root;
          package = pkgs.prek;
          hooks = {
            renovate-config-validator = {
              enable = true;
              entry = "${lib.getExe pkgs.renovate} --strict config-validator";
              files = "renovate\\.json5?$";
              pass_filenames = false;
              stages = [
                "pre-commit"
                "pre-push"
              ];
              priority = 0;
            };
            ccusage-treefmt = {
              enable = true;
              name = "treefmt";
              # `--no-cache` keeps the hook safe under prek's parallel,
              # file-batched invocations. Without it each concurrent treefmt
              # process races for the shared eval-cache SQLite db and the loser
              # dies with "failed to open cache: ... timeout" (notably during
              # `just release`, which stages every package.json at once). prek
              # already selects the changed files, so treefmt's own cache is
              # redundant here anyway.
              entry = "${lib.getExe config.treefmt.build.wrapper} --no-cache";
              files = ".*";
              pass_filenames = true;
              stages = [ "pre-commit" ];
              priority = 10;
            };
            gitleaks-protect = {
              enable = true;
              name = "gitleaks";
              entry = "${lib.getExe pkgs.gitleaks} protect --staged --config .gitleaks.toml";
              pass_filenames = false;
              always_run = true;
              stages = [ "pre-commit" ];
              priority = 20;
            };
            ccusage-treefmt-check = {
              enable = true;
              name = "treefmt";
              entry = "${lib.getExe config.treefmt.build.wrapper} --fail-on-change";
              files = ".*";
              pass_filenames = false;
              stages = [ "pre-push" ];
              priority = 0;
            };
            gitleaks-detect = {
              enable = true;
              name = "gitleaks";
              entry = "${lib.getExe pkgs.gitleaks} detect --config .gitleaks.toml";
              files = ".*";
              pass_filenames = false;
              always_run = true;
              stages = [ "pre-push" ];
              priority = 0;
            };
            ccusage-oxlint-check = {
              enable = true;
              name = "oxlint";
              entry = "${lib.getExe pkgs.oxlint} .";
              files = "\\.(ts|tsx|js|jsx|mjs|cjs)$";
              pass_filenames = false;
              stages = [ "pre-push" ];
              priority = 0;
            };
            ccusage-clippy = {
              enable = true;
              name = "clippy";
              entry = "${lib.getExe' rustToolchain "cargo"} clippy --manifest-path rust/Cargo.toml --workspace --all-targets -- -D warnings";
              files = "^rust/.*\\.rs$";
              pass_filenames = false;
              stages = [ "pre-push" ];
              priority = 0;
            };
            node-test = {
              enable = true;
              name = "node test";
              entry = "${lib.getExe pkgs.nodejs} --test apps/ccusage/src/cli.test.ts nix/tools/models-dev-gen/compact.test.ts";
              files = "\\.(ts|tsx|js|jsx|mjs|cjs)$";
              pass_filenames = false;
              stages = [ "pre-push" ];
              priority = 10;
            };
            cargo-test = {
              enable = true;
              name = "cargo test";
              entry = "${lib.getExe' rustToolchain "cargo"} test --manifest-path rust/Cargo.toml --workspace --all-targets";
              files = "^rust/(.*\\.rs|.*Cargo\\.toml|Cargo\\.lock)$";
              pass_filenames = false;
              stages = [ "pre-push" ];
              priority = 10;
            };
          };
        };
      };
    };
}
