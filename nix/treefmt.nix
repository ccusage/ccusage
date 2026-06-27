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
      craneLib = (inputs.crane.mkLib pkgs).overrideToolchain rustToolchain;
      inherit (config.packages.ccusage.passthru) cargoArtifacts commonArgs;
      generateConfigSchema = craneLib.buildPackage (
        commonArgs
        // {
          pname = "generate-config-schema";
          inherit cargoArtifacts;
          cargoExtraArgs = "-p ccusage --bin generate-config-schema";
          doCheck = false;
          meta = {
            mainProgram = "generate-config-schema";
          };
        }
      );
      schemaGen = pkgs.writeShellApplication {
        name = "ccusage-schema-gen";
        runtimeInputs = [
          pkgs.coreutils
          pkgs.diffutils
          pkgs.oxfmt
          generateConfigSchema
        ];
        # Generate the schema into a temp file and only overwrite the tracked
        # files when the content actually differs. This keeps the formatter
        # idempotent: rewriting an unchanged file bumps its mtime, which
        # `treefmt --fail-on-change` (pre-push) reports as a spurious change.
        text = ''
          tmp="$(mktemp --suffix=.json)"
          trap 'rm -f "$tmp"' EXIT
          generate-config-schema "$tmp"
          oxfmt --write "$tmp"
          if ! cmp -s "$tmp" apps/ccusage/config-schema.json; then
            cp -f "$tmp" apps/ccusage/config-schema.json
          fi
          if [ -d docs/public ] && ! cmp -s apps/ccusage/config-schema.json docs/public/config-schema.json; then
            cp -f apps/ccusage/config-schema.json docs/public/config-schema.json
          fi
        '';
      };
    in
    {
      treefmt = {
        inherit pkgs;
        projectRoot = root;
        projectRootFile = "flake.nix";

        programs = {
          deadnix.enable = true;
          nixfmt.enable = true;
          rustfmt = {
            enable = true;
            edition = "2024";
            package = rustToolchain;
          };
          statix.enable = true;
          typos = {
            enable = true;
            configFile = "./typos.toml";
          };
        };

        settings.formatter = {
          deadnix.priority = 1;
          statix.priority = 2;
          nixfmt.priority = 3;
          oxfmt = {
            command = lib.getExe pkgs.oxfmt;
            options = [ "--no-error-on-unmatched-pattern" ];
            includes = [ "*" ];
            priority = 4;
          };
          actionlint = {
            command = lib.getExe pkgs.actionlint;
            options = [
              "-ignore"
              ''unknown permission scope "code-quality"''
              "-ignore"
              "shellcheck reported issue in this script: SC2016:info:"
              # `background:` and `wait-all:` are new parallel-step keys added in
              # GitHub Actions on 2026-06-25 that actionlint does not yet recognize.
              # actionlint:ignore inline comments cannot suppress syntax-check errors
              # (only expression-evaluation and job-dependency errors support that),
              # so a global -ignore pattern is the only mechanism that works here.
              # `-ignore` matches the message text only (not the file path), so the
              # pattern cannot be narrowed to ci.yaml by prefixing the regex with a
              # filename. The risk is bounded: any step genuinely missing run:/uses:
              # would fail immediately at GitHub Actions runtime.
              "-ignore"
              ''unexpected key "background" for step''
              "-ignore"
              "step must run script with .run. section or run action with .uses. section"
            ];
            includes = [
              ".github/workflows/*.yaml"
              ".github/workflows/*.yml"
            ];
            priority = 5;
          };
          zizmor = {
            command = lib.getExe pkgs.zizmor;
            options = [
              "--offline"
              "--min-severity"
              "high"
              "--min-confidence"
              "high"
            ];
            includes = [
              ".github/workflows/*.yaml"
              ".github/workflows/*.yml"
              ".github/actions/*/action.yaml"
              ".github/actions/*/action.yml"
            ];
            priority = 6;
          };
          nufmt = {
            command = lib.getExe pkgs.nufmt;
            includes = [ "*.nu" ];
            priority = 7;
          };
          oxlint = {
            command = lib.getExe pkgs.oxlint;
            options = [
              "--fix"
              "--config"
              "nix/oxlint-check.json"
            ];
            includes = [
              "*.cjs"
              "*.js"
              "*.jsx"
              "*.mjs"
              "*.ts"
              "*.tsx"
            ];
            priority = 8;
          };
          rustfmt.priority = 9;
          schema-gen = {
            command = lib.getExe schemaGen;
            includes = [
              "apps/ccusage/config-schema.json"
              "rust/crates/ccusage/src/config_schema.rs"
              "rust/crates/ccusage/src/bin/generate_config_schema.rs"
            ];
            priority = 10;
          };
        };
      };

      # `nix run .#generate-schema` regenerates apps/ccusage/config-schema.json
      # (and the docs copy) from the current Rust source. It reuses the exact
      # script the treefmt formatter and the config-schema flake check rely on,
      # so the three can never drift apart. Run it from the repo root.
      apps.generate-schema = {
        type = "app";
        program = lib.getExe schemaGen;
      };
    };
}
