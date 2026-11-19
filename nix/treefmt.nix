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
      # The generator only needs the config layer, so it starts from the foundation
      # artifacts rather than the adapter ones: this derivation gates the CI
      # preflight, and waiting for 15 adapters there would delay every build job.
      cargoArtifacts = workspaceArtifacts.foundation;
      generateConfigSchema = craneLib.buildPackage (
        commonArgs
        // {
          pname = "generate-config-schema";
          inherit cargoArtifacts;
          cargoExtraArgs = "-p ccusage-config --bin generate-config-schema";
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
        # Rewriting an unchanged file bumps its mtime, which
        # `treefmt --fail-on-change` reports as a change.
        text = ''
          tmp="$(mktemp --suffix=.json)"
          trap 'rm -f "$tmp"' EXIT
          generate-config-schema "$tmp"
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

        # The generated pricing snapshots carry upstream model ids verbatim, and
        # some read as misspellings — one Gemini id ends in a clipped "no
        # thinking". typos rewrites those to the word it expects, which silently
        # stops the model from ever matching, and it rewrites them in this
        # comment too if they are spelled out here. Only typos is excluded,
        # because the JSON formatter is what keeps these files reviewable.
        settings.formatter.typos.excludes = [
          "rust/crates/ccusage-pricing-snapshots/src/models-dev-*.json"
          # Narrow table snapshots clip cell text mid-word, and typos would
          # "correct" the clipped fragments and so rewrite the expected output.
          "rust/crates/ccusage/tests/snapshots/golden__*table*.snap"
        ];

        # The tagpr PR template is a Go text/template, and oxfmt's markdown
        # rewrites break its <details> block and nested list structure.
        #
        # `bun.lock`/`bun.nix` under nix/tools are regenerated verbatim by `bun
        # install` and `bun2nix`. Formatting them fights the generators: oxfmt
        # rewrites the JSONC lockfile, and deadnix strips the unused arguments
        # that bun.nix's `callPackage` signature requires.
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
              # treefmt batches the files it matched and hands them over as
              # arguments, so a batch can consist entirely of paths that
              # oxlint-check.json ignores. oxlint treats "nothing left to lint"
              # as an error, which surfaces as a formatter failure for a file
              # that was deliberately excluded.
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
            includes = [
              "apps/ccusage/config-schema.json"
              "rust/crates/ccusage-config/src/config_schema.rs"
              "rust/crates/ccusage-config/src/bin/generate_config_schema.rs"
            ];
            priority = 10;
          };
        };
      };

      # `nix run .#generate-schema` regenerates apps/ccusage/config-schema.json
      # from the current Rust source with the script the treefmt formatter
      # runs, so `checks.treefmt` fails when the committed file drifts. Run it
      # from the repo root.
      apps.generate-schema = {
        type = "app";
        program = lib.getExe schemaGen;
      };
      # `nix run .#generate-bun-nix` derives every committed bun.nix from its
      # sibling bun.lock. autofix.ci runs it on every pull request, while
      # contributors can use `just gen-bun-nix` when a manifest also needs Bun
      # to resolve a new lockfile.
      apps.generate-bun-nix = {
        type = "app";
        program = lib.getExe generateBunNix;
      };
    };
}
