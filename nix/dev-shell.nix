{ inputs, lib, ... }:
{
  perSystem =
    {
      config,
      system,
      pkgs,
      rustToolchain,
      ...
    }:
    let
      gitWtHook = import ./git-wt.nix { inherit pkgs; };
    in
    {
      devShells.default = pkgs.mkShell {
        CCUSAGE_PRICING_JSON_PATH = "${inputs.litellm}";
        CCUSAGE_SCHEMA_GEN_FROM_WORKSPACE = "1";

        RUSTFLAGS = config.packages.ccusage.passthru.linkerRustflags;

        buildInputs =
          (with pkgs; [
            nodejs
            config.packages.pnpm
            config.packages.tirith
            bun
            inputs.bun2nix.packages.${system}.default
            nushell
            config.packages.publint

            rustToolchain
            config.packages.cargo-hawk
            cargo-edit
            cargo-insta
            (cargo-llvm-cov.overrideAttrs { doCheck = false; })
            pkg-config
            openssl
            config.treefmt.build.wrapper
            nixfmt
            deadnix
            statix
            typos
            typos-lsp
            oxfmt
            config.packages.actionlint
            config.packages.blacksmith
            zizmor
            config.packages.oxlint
            just
            clj-kondo
            prek
            gitleaks
            renovate
            jq
            git
            git-wt
            gh
            hyperfine
            similarity
            ast-grep
            ripgrep
            fd
            fzf
            delta
            dust
          ])
          ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [
            pkgs.mold
          ]
          ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [
            pkgs.apple-sdk_15
            pkgs.llvmPackages.lld
          ]
          ++ config.pre-commit.settings.enabledPackages;

        shellHook = ''
          export RUSTC_WRAPPER="''${RUSTC_WRAPPER-${lib.getExe pkgs.sccache}}"

          just install if-stale

          ${lib.getExe config.packages.syncAgentSkills}
          ${config.pre-commit.shellHook}

          for stage in ${lib.escapeShellArgs config.pre-commit.settings.installStages}; do
            hookFile=$(${lib.getExe config.pre-commit.settings.gitPackage} rev-parse --path-format=absolute --git-path "hooks/$stage")
            if ! grep -q -- --skip-on-missing-config "$hookFile" 2>/dev/null; then
              ${lib.getExe config.pre-commit.settings.package} install \
                -c ${config.pre-commit.settings.configPath} -t "$stage" --allow-missing-config >/dev/null
            fi
          done
        ''
        + gitWtHook;
      };
    };
}
