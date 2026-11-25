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
      mkSchemaGen =
        {
          name,
          crate,
          bin,
          features ? [ ],
          output,
        }:
        let
          featureArgs = lib.optionalString (
            features != [ ]
          ) " --features ${lib.concatStringsSep "," features}";
          generator = craneLib.buildPackage (
            commonArgs
            // {
              pname = bin;
              inherit cargoArtifacts;
              cargoExtraArgs = "-p ${crate} --bin ${bin}${featureArgs}";
              doCheck = false;
              meta.mainProgram = bin;
            }
          );
        in
        pkgs.writeShellApplication {
          inherit name;
          runtimeInputs = [
            pkgs.coreutils
            pkgs.diffutils
            pkgs.oxfmt
            generator
          ];
          text = ''
            tmp="$(mktemp --suffix=.${lib.last (lib.splitString "." output)})"
            trap 'rm -f "$tmp"' EXIT
            # NOTE(2026-11-20, #2343): a dev shell reuses this wrapper from its last
            # evaluation, so a baked-in generator misses newer fields. The dev
            # shell sets this variable to build from the working tree; the pure flake
            # check leaves it unset and keeps the store-built generator.
            if [[ -n "''${CCUSAGE_SCHEMA_GEN_FROM_WORKSPACE:-}" ]]; then
              cargo run --locked --quiet --manifest-path rust/Cargo.toml \
                -p ${crate}${featureArgs} --bin ${bin} -- "$tmp"
            else
              ${bin} "$tmp"
            fi
            oxfmt --write "$tmp"
            cmp -s "$tmp" ${output} || cp "$tmp" ${output}
          '';
        };
      schemaGen = mkSchemaGen {
        name = "ccusage-schema-gen";
        crate = "ccusage-config";
        bin = "generate-config-schema";
        features = [ "schema" ];
        output = "apps/ccusage/config-schema.json";
      };
      telemetrySchemaGen = mkSchemaGen {
        name = "ccusage-telemetry-schema-gen";
        crate = "ccusage-telemetry";
        bin = "generate-telemetry-schema";
        output = "telemetry/src/v1/ping-schema.gen.ts";
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
          telemetry-schema-gen = {
            command = lib.getExe telemetrySchemaGen;
            includes = [
              "telemetry/src/v1/ping-schema.gen.ts"
              "rust/crates/ccusage-telemetry/src/*.rs"
              "rust/crates/ccusage-telemetry/src/bin/*.rs"
            ];
            priority = 10;
          };
        };
      };

      apps.generate-schema = {
        type = "app";
        program = lib.getExe schemaGen;
      };
      apps.generate-telemetry-schema = {
        type = "app";
        program = lib.getExe telemetrySchemaGen;
      };
      apps.generate-bun-nix = {
        type = "app";
        program = lib.getExe generateBunNix;
      };
    };
}
