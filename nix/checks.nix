{ inputs, lib, ... }:
let
  root = ./..;
in
{
  perSystem =
    {
      config,
      pkgs,
      craneLib,
      testArtifacts,
      ...
    }:
    let
      inherit (config.packages.ccusage.passthru)
        commonArgs
        version
        ;
      inherit (testArtifacts) cargoArtifacts;
      rustSrc = import ./rust-src.nix { inherit lib root; };
      repoSrc = lib.cleanSourceWith {
        name = "source";
        src = root;
        filter =
          path: type:
          !(
            type == "directory"
            && builtins.elem (baseNameOf path) [
              "node_modules"
              "target"
              "dist"
              "coverage"
            ]
          );
      };
      clippyArgs = {
        cargoExtraArgs = "--workspace";
        cargoClippyExtraArgs = "--all-targets --config 'build.warnings=\"deny\"'";
      };
      ccusage-clippy = craneLib.cargoClippy (
        commonArgs
        // testArtifacts.consumerArgs
        // clippyArgs
        // {
          src = rustSrc;
          sourceRoot = "source/rust";
          cargoLock = root + /rust/Cargo.lock;
          inherit cargoArtifacts;
          passthru = { inherit clippyArgs; };
        }
      );
      ccusage-hawk = craneLib.mkCargoDerivation (
        commonArgs
        // testArtifacts.consumerArgs
        // {
          pname = "ccusage-hawk";
          src = rustSrc;
          sourceRoot = "source/rust";
          cargoLock = root + /rust/Cargo.lock;
          cargoArtifacts = testArtifacts.hawkArtifacts;
          nativeBuildInputs = commonArgs.nativeBuildInputs or [ ] ++ [ config.packages.cargo-hawk ];
          buildPhaseCargoCommand = ''
            cargo hawk check --manifest-path Cargo.toml --target-dir "''${CARGO_TARGET_DIR:-target}" -D warnings
          '';
        }
      );
      ccusage-unsafe-code = import ./unsafe-code-check.nix {
        inherit
          commonArgs
          craneLib
          lib
          root
          ;
        inherit (pkgs) nushell;
        inherit cargoArtifacts;
        withScriptTests = true;
      };
      ccusage-fmt = craneLib.cargoFmt {
        pname = "ccusage-rust";
        inherit version;
        src = rustSrc;
        sourceRoot = "source/rust";
        cargoExtraArgs = "--all";
      };
      generateTelemetrySchema = craneLib.buildPackage (
        commonArgs
        // {
          pname = "generate-telemetry-schema";
          inherit cargoArtifacts;
          cargoExtraArgs = "-p ccusage-telemetry --bin generate-telemetry-schema";
          doCheck = false;
          meta = {
            mainProgram = "generate-telemetry-schema";
          };
        }
      );
      telemetry-schema =
        pkgs.runCommand "telemetry-schema-check"
          {
            src = repoSrc;
            nativeBuildInputs = [
              generateTelemetrySchema
              pkgs.oxfmt
              pkgs.diffutils
            ];
          }
          ''
            cp -R "$src" source
            chmod -R u+w source
            cd source

            generate-telemetry-schema generated.ts
            oxfmt --write generated.ts

            if ! diff -u apps/telemetry/src/v1/ping-schema.gen.ts generated.ts; then
              echo "ERROR: apps/telemetry/src/v1/ping-schema.gen.ts is out of sync with the Rust Ping type." >&2
              echo "Run 'just telemetry::generate-schema' and commit the result." >&2
              exit 1
            fi

            touch "$out"
          '';
      bun-nix =
        mkRepoCheck "bun-nix-check"
          [
            config.packages.generate-bun-nix
            pkgs.diffutils
          ]
          ''
            generate-bun-nix
            diff -ru "$src/nix/tools" nix/tools
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
      checks = {
        clj-kondo = mkRepoCheck "clj-kondo-check" [ pkgs.clj-kondo pkgs.fd ] ''
          fd --hidden --exclude .git --type file --extension clj --extension cljc --extension bb --exec-batch clj-kondo --lint
        '';
        ccusage-package = config.packages.ccusage;
        ccusage-tests = config.packages.ccusage-tests;
        clippy-config = mkRepoCheck "clippy-config-check" [ pkgs.nushell ] ''
          nu --no-config-file scripts/check-clippy-config.nu rust
        '';
        models-dev-dependencies = mkRepoCheck "models-dev-dependencies-check" [ pkgs.nushell ] ''
          nu scripts/check-models-dev-dependencies.nu \
            ${inputs.models-dev}/bun.lock nix/tools/models-dev-gen/package.json
        '';
        workspace-dependency-inheritance =
          mkRepoCheck "workspace-dependency-inheritance-check" [ pkgs.nushell ]
            ''
              nu --no-config-file scripts/check-workspace-dependency-inheritance.nu rust
            '';
        comments = mkRepoCheck "comment-check" [ pkgs.nushell pkgs.ast-grep pkgs.just ] ''
          nu --no-config-file scripts/comments/check.nu --source-tree
        '';
        infra-scripts =
          mkRepoCheck "infra-script-tests"
            [
              pkgs.ast-grep
              pkgs.nushell
              pkgs.just
              pkgs.jq
              pkgs.git
            ]
            ''
              nu --no-config-file scripts/comments/tests/check.test.nu
              nu scripts/validate-commit-scope.test.nu
              nu scripts/check-models-dev-dependencies.test.nu
              nu scripts/check-workspace-dependency-inheritance.test.nu
              nu scripts/check-nushell.test.nu
              nu apps/ccusage/scripts/verify-native-package.test.nu
              nu .github/scripts/contribution-gate.test.nu
              nu .github/scripts/pricing-lock.test.nu
              nu .github/scripts/upsert-pr-comment.test.nu
              nu .github/scripts/check-links.test.nu
              nu .github/scripts/profile-instructions.test.nu
              nu .github/scripts/binary-size.test.nu
              nu .github/scripts/workflow-shell.test.nu
              nu .github/scripts/install.test.nu
            '';
        inherit
          bun-nix
          ccusage-clippy
          ccusage-fmt
          ccusage-hawk
          telemetry-schema
          ccusage-unsafe-code
          ;
        oxlint = mkRepoCheck "oxlint-check" [ config.packages.oxlint ] ''
          oxlint --config nix/oxlint-check.json .
        '';
        gitleaks = mkRepoCheck "gitleaks-check" [ pkgs.gitleaks ] ''
          gitleaks detect --source . --config .gitleaks.toml --no-git
        '';
        nushell-ide-check = mkRepoCheck "nushell-ide-check" [ pkgs.nushell ] ''
          nu --no-config-file scripts/check-nushell.nu .
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
              mapfile -t packageManifests < <(fd --type f '^package\.json$' . --exclude nix/tools)

              node - "''${packageManifests[@]}" <<'EOF'
              const fs = require("node:fs");
              const path = require("node:path");

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
                if (path.normalize(manifestPath).startsWith("packages/")) {
                  for (const relativePath of packageJson.files ?? []) {
                    if (relativePath.startsWith("bin/")) {
                      touchGeneratedFile(packageDir, relativePath);
                    }
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
