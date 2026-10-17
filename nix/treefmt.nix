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
        # Generate the schema into a temp file and only overwrite the tracked
        # files when the content actually differs. This keeps the formatter
        # idempotent: rewriting an unchanged file bumps its mtime, which
        # `treefmt --fail-on-change` (pre-push) reports as a spurious change.
        # treefmt may run this formatter as several concurrent batches, so a
        # plain `cp` into a missing destination can lose the create race with
        # "File exists". Each copy is staged beside its destination and renamed
        # into place, which is atomic and never conflicts.
        text = ''
          tmp="$(mktemp --suffix=.json)"
          staged=""
          trap 'rm -f "$tmp" "$staged"' EXIT
          install_if_changed() {
            if cmp -s "$1" "$2"; then
              return
            fi
            staged="$(mktemp "$2.XXXXXX")"
            cp "$1" "$staged"
            # mktemp creates the file 0600; match the tracked file's mode.
            chmod 0644 "$staged"
            mv -f "$staged" "$2"
            staged=""
          }
          generate-config-schema "$tmp"
          oxfmt --write "$tmp"
          install_if_changed "$tmp" apps/ccusage/config-schema.json
          if [ -d docs/public ]; then
            install_if_changed apps/ccusage/config-schema.json docs/public/config-schema.json
          fi
        '';
      };
      generateBunNix = pkgs.writeShellApplication {
        name = "generate-bun-nix";
        runtimeInputs = [
          inputs.bun2nix.packages.${system}.default
          pkgs.coreutils
        ];
        text = ''
          if [ "$#" -eq 0 ]; then
            set -- nix/tools/*/bun.lock
          fi
          for lockfile in "$@"; do
            toolDir="$(dirname "$lockfile")"
            echo "Regenerating $toolDir"
            (cd "$toolDir" && bun2nix -o bun.nix)
          done
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

        # The generated pricing snapshots carry upstream model ids verbatim, and
        # some read as misspellings — one Gemini id ends in a clipped "no
        # thinking". typos rewrites those to the word it expects, which silently
        # stops the model from ever matching, and it rewrites them in this
        # comment too if they are spelled out here. Only typos is excluded,
        # because the JSON formatter is what keeps these files reviewable.
        settings.formatter.typos.excludes = [
          "rust/crates/ccusage-core/src/models-dev-pricing.json"
          "rust/crates/ccusage-core/src/models-dev-catalog-rules.json"
          "rust/adapters/codex/src/codex-auto-review-fallbacks.json"
          "nix/tools/*/bun.lock"
        ];

        # The tagpr PR template is a Go text/template, and oxfmt's markdown
        # rewrites break its <details> block and nested list structure.
        #
        # `bun.lock`/`bun.nix` under nix/tools are regenerated verbatim by `bun
        # install` and `bun2nix`. Formatting them fights the generators: oxfmt
        # rewrites the JSONC lockfile, and deadnix strips the unused arguments
        # that bun.nix's `callPackage` signature requires. `bun.lock` stays
        # visible to the bun-nix formatter, which derives `bun.nix` from it.
        settings.global.excludes = [
          ".github/tagpr-template.md"
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
            excludes = [ "nix/tools/*/bun.lock" ];
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
          bun-nix = {
            command = lib.getExe generateBunNix;
            includes = [ "nix/tools/*/bun.lock" ];
            priority = 11;
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
      # `nix run .#generate-bun-nix` derives every committed bun.nix from its
      # sibling bun.lock. It is also the bun-nix treefmt formatter, so autofix.ci
      # commits the result on dependency PRs; contributors can use
      # `just gen-bun-nix` when a manifest also needs Bun to resolve a new lockfile.
      apps.generate-bun-nix = {
        type = "app";
        program = lib.getExe generateBunNix;
      };
    };
}
