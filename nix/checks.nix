{ inputs, ... }:
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
      inherit (config.packages.ccusage.passthru)
        cargoArtifacts
        workspaceArtifacts
        commonArgs
        version
        ;
      nixFilter = inputs.nix-filter.lib;
      rustSrc = import ./rust-src.nix { inherit nixFilter root; };
      repoSrc = nixFilter {
        inherit root;
        exclude = [
          (nixFilter.matchName "node_modules")
          (nixFilter.matchName "target")
          (nixFilter.matchName "dist")
          (nixFilter.matchName "coverage")
        ];
      };
      ccusage-clippy = craneLib.cargoClippy (
        commonArgs
        // {
          src = rustSrc;
          sourceRoot = "source/rust";
          cargoLock = root + /rust/Cargo.lock;
          inherit cargoArtifacts;
          cargoExtraArgs = "--workspace";
          cargoClippyExtraArgs = "--all-targets -- -D warnings";
        }
      );
      ccusage-fmt = craneLib.cargoFmt {
        pname = "ccusage-rust";
        inherit version;
        src = rustSrc;
        sourceRoot = "source/rust";
        cargoExtraArgs = "--all";
      };
      # Build the schema generator straight from the current Rust source so the
      # drift check below can never go stale: it always reflects whatever
      # `rust/crates/ccusage/src/config_schema.rs` looks like right now.
      generateConfigSchema = craneLib.buildPackage (
        commonArgs
        // {
          pname = "generate-config-schema";
          # Only the config layer is needed, so skip the adapter artifacts.
          cargoArtifacts = workspaceArtifacts.foundation;
          cargoExtraArgs = "-p ccusage-config --bin generate-config-schema";
          doCheck = false;
          meta = {
            mainProgram = "generate-config-schema";
          };
        }
      );
      # Fail `nix flake check` when the committed config schema drifts from what
      # the Rust source generates. This catches PRs that add or change a config
      # field without regenerating the schema (run
      # `just ccusage::generate-schema` to fix). Only the tracked
      # apps/ccusage/config-schema.json is checked; docs/public/config-schema.json
      # is a gitignored build copy.
      config-schema =
        pkgs.runCommand "config-schema-check"
          {
            src = repoSrc;
            nativeBuildInputs = [
              generateConfigSchema
              pkgs.oxfmt
              pkgs.diffutils
            ];
          }
          ''
            cp -R "$src" source
            chmod -R u+w source
            cd source

            generate-config-schema generated.json
            oxfmt --write generated.json

            if ! diff -u apps/ccusage/config-schema.json generated.json; then
              echo "ERROR: apps/ccusage/config-schema.json is out of sync with the Rust schema source." >&2
              echo "Run 'nix run .#generate-schema' (or 'just ccusage::generate-schema') and commit the result." >&2
              exit 1
            fi

            touch "$out"
          '';
      # Fail `nix flake check` when a tool's committed `bun.nix` no longer matches
      # what `bun2nix` derives from its `bun.lock`. Renovate bumps `bun.lock`
      # without knowing about `bun.nix`, and a stale pair would otherwise only
      # surface as a confusing sandbox install failure. `bun2nix` only parses the
      # lockfile, so this needs no network access.
      bun-nix =
        mkRepoCheck "bun-nix-check"
          [
            inputs.bun2nix.packages.${system}.default
            pkgs.diffutils
          ]
          ''
            for lockfile in nix/tools/*/bun.lock; do
              toolDir="$(dirname "$lockfile")"
              (cd "$toolDir" && bun2nix -o generated.nix)
              if ! diff -u "$toolDir/bun.nix" "$toolDir/generated.nix"; then
                echo "ERROR: $toolDir/bun.nix is out of sync with $lockfile." >&2
                echo "Run 'just gen-bun-nix' and commit the result." >&2
                exit 1
              fi
            done
          '';
      mkRepoCheck =
        name: nativeBuildInputs: command:
        pkgs.runCommand name
          {
            src = repoSrc;
            inherit nativeBuildInputs;
          }
          ''
            cp -R "$src" source
            chmod -R u+w source
            cd source
            ${command}
            touch "$out"
          '';
    in
    {
      # `nix flake check` deliberately omits a full `config.packages.ccusage`
      # build: ccusage-clippy already compiles the entire workspace with
      # `--all-targets` (catching any build break), and the build-native-packages
      # CI job produces and verifies the actual release binary. Building the
      # optimized native package here too only duplicated a ~70s release compile
      # on cache-cold runners.
      checks = {
        inherit
          bun-nix
          ccusage-clippy
          ccusage-fmt
          config-schema
          ;
        oxlint = mkRepoCheck "oxlint-check" [ pkgs.oxlint ] ''
          oxlint --config nix/oxlint-check.json .
        '';
        gitleaks = mkRepoCheck "gitleaks-check" [ pkgs.gitleaks ] ''
          gitleaks detect --source . --config .gitleaks.toml --no-git
        '';
        config-example = mkRepoCheck "config-example-check" [ pkgs.check-jsonschema ] ''
          check-jsonschema --schemafile apps/ccusage/config-schema.json ccusage.example.json
        '';
        publint =
          mkRepoCheck "publint-check"
            [
              config.packages.publint
              pkgs.fd
              pkgs.nodejs
            ]
            ''
              # nix/tools/*/package.json are dependency manifests for Nix-built
              # JS tooling, not publishable packages, so they are out of scope.
              mapfile -t packageManifests < <(fd --type f '^package\.json$' . --exclude nix/tools)

              node - "''${packageManifests[@]}" <<'EOF'
              const fs = require("node:fs");
              const path = require("node:path");

              const generatedArtifacts = new Map([
                ["@ccusage/ccusage-darwin-arm64", ["bin/ccusage"]],
                ["@ccusage/ccusage-darwin-x64", ["bin/ccusage"]],
                ["@ccusage/ccusage-linux-arm64", ["bin/ccusage"]],
                ["@ccusage/ccusage-linux-x64", ["bin/ccusage"]],
                ["@ccusage/ccusage-win32-arm64", ["bin/ccusage.exe"]],
                ["@ccusage/ccusage-win32-x64", ["bin/ccusage.exe"]],
              ]);

              function touchGeneratedFile(packageDir, relativePath) {
                const targetPath = path.join(packageDir, relativePath);
                fs.mkdirSync(path.dirname(targetPath), { recursive: true });
                if (!fs.existsSync(targetPath)) {
                  fs.writeFileSync(targetPath, "");
                }
                if (!relativePath.endsWith(".exe")) {
                  fs.chmodSync(targetPath, 0o755);
                }
              }

              for (const manifestPath of process.argv.slice(2)) {
                const packageDir = path.dirname(manifestPath);
                const packageJson = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
                const files = new Set(packageJson.files ?? []);
                for (const relativePath of generatedArtifacts.get(packageJson.name) ?? []) {
                  if (files.has(relativePath)) {
                    touchGeneratedFile(packageDir, relativePath);
                  }
                }
              }
              EOF

              for packageJson in "''${packageManifests[@]}"; do
                packageDir="$(dirname "$packageJson")"
                echo "Running publint for $packageDir"
                publint run "$packageDir" --pack false
              done
            '';
      };
    };
}
