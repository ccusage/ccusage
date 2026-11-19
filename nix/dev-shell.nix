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
        # Hand cargo the pinned LiteLLM snapshot the way the Nix packages do, so
        # `cargo build` in the dev shell stays offline and does not need the
        # fetch-litellm-pricing feature's rustls stack.
        CCUSAGE_PRICING_JSON_PATH = "${inputs.litellm}";

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
            # nixpkgs' rustc bundles an LLVM 23 profiler runtime while being
            # built against LLVM 21, so the instrumented binaries in this
            # package's own test suite segfault at exit. The shell runs
            # cargo-llvm-cov with rustToolchain, which is unaffected. Drop the
            # override once https://github.com/NixOS/nixpkgs/pull/578990
            # reaches the pinned nixpkgs (https://github.com/NixOS/nixpkgs/issues/576588).
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
          # Every worktree starts with an empty rust/target, so without a shared
          # cache each one recompiles all dependencies, C build scripts included
          # (cc-rs routes those through sccache too). Cached dependencies cut a
          # fresh worktree's `cargo test --no-run` by about a quarter; workspace
          # crates are incremental, so sccache passes them straight to rustc.
          # An existing RUSTC_WRAPPER, even an empty one, wins as an opt-out.
          export RUSTC_WRAPPER="''${RUSTC_WRAPPER-${lib.getExe pkgs.sccache}}"

          just install if-stale

          ${lib.getExe config.packages.syncAgentSkills}
          ${config.pre-commit.shellHook}

          # Hooks are shared across worktrees, but the config symlink only exists
          # where this shell has loaded; git-hooks.nix cannot pass this flag.
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
