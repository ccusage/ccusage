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
      pkgs,
      rustToolchain,
      craneLib,
      ...
    }:
    let
      inherit (config.packages.ccusage.passthru) commonArgs workspaceArtifacts;
      cargoArtifacts = workspaceArtifacts.foundation;
      generateConfigSchema = craneLib.buildPackage (
        commonArgs
        // {
          pname = "generate-config-schema";
          inherit cargoArtifacts;
          cargoExtraArgs = "-p ccusage-config --bin generate-config-schema --features schema";
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
        text = ''
          tmp="$(mktemp --suffix=.json)"
          trap 'rm -f "$tmp"' EXIT
          # NOTE(2026-11-20, #2343): a dev shell reuses this wrapper from its last
          # evaluation, so a baked-in generator misses newer config fields. The dev
          # shell sets this variable to build from the working tree; the pure flake
          # check leaves it unset and keeps the store-built generator.
          if [[ -n "''${CCUSAGE_SCHEMA_GEN_FROM_WORKSPACE:-}" ]]; then
            cargo run --locked --manifest-path rust/Cargo.toml \
              -p ccusage-config --features schema --bin generate-config-schema -- "$tmp"
          else
            generate-config-schema "$tmp"
          fi
          oxfmt --write "$tmp"
          cmp -s "$tmp" apps/ccusage/config-schema.json || cp "$tmp" apps/ccusage/config-schema.json
        '';
      };
      generateBunNix = pkgs.writeShellApplication {
        name = "generate-bun-nix";
        runtimeInputs = [
          inputs.bun2nix.packages.${system}.default
          pkgs.coreutils
        ];
        text = ''
          for lockfile in nix/tools/*/bun.lock; do
            toolDir="$(dirname "$lockfile")"
            echo "Regenerating $toolDir"
            (cd "$toolDir" && bun2nix -o bun.nix)
          done
        '';
      };
    in
    {
      packages.generate-bun-nix = generateBunNix;
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

        settings.formatter.typos.excludes = [
          "rust/crates/ccusage-pricing-snapshots/src/models-dev-*.json"
          "rust/crates/ccusage/tests/snapshots/golden__*table*.snap"
        ];

        settings.global.excludes = [
          ".github/tagpr-template.md"
          "nix/tools/*/bun.lock"
          "nix/tools/*/bun.nix"
        ];

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
            command = lib.getExe config.packages.actionlint;
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
            command = lib.getExe config.packages.oxlint;
            options = [
              "--fix"
              "--config"
              "nix/oxlint-check.json"
              "--no-error-on-unmatched-pattern"
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
            # NOTE(2026-11-20, #2361): the schema also depends on the option structs in `config_schema` and the `ccusage-cli` agent specs. The includes are listed by hand because treefmt cannot derive the Rust module graph.
            includes = [
              "apps/ccusage/config-schema.json"
              "rust/crates/ccusage-cli/src/*.rs"
              "rust/crates/ccusage-config/src/bin/generate_config_schema.rs"
              "rust/crates/ccusage-config/src/config_schema.rs"
              "rust/crates/ccusage-config/src/config_schema/*.rs"
            ];
            priority = 10;
          };
        };
      };

      apps.generate-schema = {
        type = "app";
        program = lib.getExe schemaGen;
      };
      apps.generate-bun-nix = {
        type = "app";
        program = lib.getExe generateBunNix;
      };
    };
}
