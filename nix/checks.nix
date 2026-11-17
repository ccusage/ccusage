{ inputs, lib, ... }:
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
        commonArgs
        depsOnlyArgs
        version
        ;
      testArtifacts = import ./test-artifacts.nix {
        inherit craneLib depsOnlyArgs;
      };
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
          # The pre-push hook runs the same lint outside Nix.
          passthru = { inherit clippyArgs; };
        }
      );
      ccusage-fmt = craneLib.cargoFmt {
        pname = "ccusage-rust";
        inherit version;
        src = rustSrc;
        sourceRoot = "source/rust";
        cargoExtraArgs = "--all";
      };
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
      # `nix flake check` deliberately omits a full `config.packages.ccusage`
      # build: ccusage-clippy already compiles the entire workspace with
      # `--all-targets` (catching any build break), and the build-native-packages
      # CI job produces and verifies the actual release binary. Building the
      # optimized native package here too only duplicated a ~70s release compile
      # on cache-cold runners.
      checks = {
        ccusage-tests = config.packages.ccusage-tests;
        models-dev-dependencies = mkRepoCheck "models-dev-dependencies-check" [ pkgs.nushell ] ''
          nu scripts/check-models-dev-dependencies.nu \
            ${inputs.models-dev}/bun.lock nix/tools/models-dev-gen/package.json
        '';
        infra-scripts = mkRepoCheck "infra-script-tests" [ pkgs.nushell pkgs.just pkgs.jq ] ''
          nu scripts/validate-commit-scope.test.nu
          nu scripts/check-models-dev-dependencies.test.nu
          nu .github/scripts/contribution-gate.test.nu
          nu .github/scripts/pricing-lock.test.nu
          nu .github/scripts/upsert-pr-comment.test.nu
          nu .github/scripts/check-links.test.nu
          nu .github/scripts/workflow-shell.test.nu
          nu .github/scripts/install.test.nu
        '';
        inherit
          bun-nix
          ccusage-clippy
          ccusage-fmt
          ;
        oxlint = mkRepoCheck "oxlint-check" [ pkgs.oxlint ] ''
          oxlint --config nix/oxlint-check.json .
        '';
        gitleaks = mkRepoCheck "gitleaks-check" [ pkgs.gitleaks ] ''
          gitleaks detect --source . --config .gitleaks.toml --no-git
        '';
        # CI jobs that need only Nushell install it without Nix, so the
        # version is pinned in the action and checked against the dev shell.
        nushell-version = mkRepoCheck "nushell-version-check" [ ] ''
          if ! grep -qxF '        version: ${pkgs.nushell.version}' .github/actions/setup-nu/action.yaml; then
            echo "ERROR: .github/actions/setup-nu must install Nushell ${pkgs.nushell.version}, the flake's version." >&2
            exit 1
          fi
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
