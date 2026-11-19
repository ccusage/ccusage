{
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
      pkgs,
      rustToolchain,
      ...
    }:
    let
      hookCommand =
        name: runtimeInputs: text:
        lib.getExe (
          pkgs.writeShellApplication {
            name = "ccusage-hook-${name}";
            inherit runtimeInputs text;
          }
        );
      inherit (config.checks.ccusage-clippy.passthru.clippyArgs) cargoExtraArgs cargoClippyExtraArgs;
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
              entry = "${pkgs.renovate}/bin/renovate-config-validator --strict";
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
              entry = lib.getExe config.treefmt.build.wrapper;
              files = ".*";
              pass_filenames = true;
              require_serial = true;
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
            ccusage-commit-scope = {
              enable = true;
              name = "commit scope";
              entry = "${lib.getExe pkgs.nushell} scripts/validate-commit-scope.nu";
              pass_filenames = true;
              always_run = true;
              stages = [ "commit-msg" ];
              priority = 0;
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
              entry = "${lib.getExe config.packages.oxlint} .";
              files = "\\.(ts|tsx|js|jsx|mjs|cjs)$";
              pass_filenames = false;
              stages = [ "pre-push" ];
              priority = 0;
            };
            ccusage-clippy = {
              enable = true;
              name = "clippy";
              entry = hookCommand "clippy" [
                rustToolchain
              ] "cargo clippy --manifest-path rust/Cargo.toml ${cargoExtraArgs} ${cargoClippyExtraArgs}";
              files = "^rust/.*\\.rs$";
              pass_filenames = false;
              stages = [ "pre-push" ];
              priority = 0;
            };
            ccusage-comments = {
              enable = true;
              name = "comments";
              entry =
                hookCommand "comments"
                  [
                    pkgs.nushell
                    pkgs.ast-grep
                    pkgs.just
                    pkgs.git
                    pkgs.gnutar
                  ]
                  ''
                    if [[ -n "''${PRE_COMMIT_TO_REF:-}" ]]; then
                      exec nu --no-config-file scripts/comments/check.nu --ref "$PRE_COMMIT_TO_REF"
                    fi
                    exec nu --no-config-file scripts/comments/check.nu
                  '';
              pass_filenames = false;
              always_run = true;
              stages = [ "pre-push" ];
              priority = 0;
            };
            node-test = {
              enable = true;
              name = "node test";
              entry = hookCommand "node-test" [
                pkgs.just
                pkgs.nodejs
              ] "just test-node";
              files = "\\.(ts|tsx|js|jsx|mjs|cjs)$";
              pass_filenames = false;
              stages = [ "pre-push" ];
              priority = 10;
            };
            cargo-test = {
              enable = true;
              name = "cargo test";
              entry = hookCommand "cargo-test" [
                pkgs.just
                rustToolchain
              ] "just rust::test";
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
