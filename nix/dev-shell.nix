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
      gitWtHook = import ./git-wt.nix { inherit pkgs; };
    in
    {
      devShells.default = pkgs.mkShell {
        # Hand cargo the pinned LiteLLM snapshot the way the Nix packages do, so
        # `cargo build` in the dev shell stays offline and does not need the
        # fetch-litellm-pricing feature's rustls stack.
        CCUSAGE_PRICING_JSON_PATH = "${inputs.litellm}";

        buildInputs =
          (with pkgs; [
            nodejs
            config.packages.pnpm
            config.packages.tirith
            bun
            inputs.bun2nix.packages.${system}.default
            nushell
            config.packages.cargo-hawk
            config.packages.publint

            rustToolchain
            cargo-edit
            cargo-insta
            cargo-llvm-cov
            pkg-config
            openssl
            config.treefmt.build.wrapper
            nixfmt
            deadnix
            statix
            typos
            typos-lsp
            oxfmt
            actionlint
            zizmor
            oxlint
            just
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
          if [ "$(uname -s)" = "Linux" ]; then
            case " ''${RUSTFLAGS:-} " in
              *" -C link-arg=-fuse-ld=mold "*) ;;
              *) export RUSTFLAGS="''${RUSTFLAGS:+$RUSTFLAGS }-C link-arg=-fuse-ld=mold" ;;
            esac
          fi
          # A test run relinks every test binary downstream of an edit, and
          # lld's Mach-O port links them faster than the cctools ld64 the Nix
          # clang wrapper defaults to, the same reason Linux uses mold above.
          if [ "$(uname -s)" = "Darwin" ]; then
            case " ''${RUSTFLAGS:-} " in
              *" -C link-arg=-fuse-ld=lld "*) ;;
              *) export RUSTFLAGS="''${RUSTFLAGS:+$RUSTFLAGS }-C link-arg=-fuse-ld=lld" ;;
            esac
          fi

          # Every worktree starts with an empty rust/target, so without a shared
          # cache each one recompiles all dependencies, C build scripts included
          # (cc-rs routes those through sccache too). Cached dependencies cut a
          # fresh worktree's `cargo test --no-run` by about a quarter; workspace
          # crates are incremental, so sccache passes them straight to rustc.
          # An existing RUSTC_WRAPPER, even an empty one, wins as an opt-out.
          export RUSTC_WRAPPER="''${RUSTC_WRAPPER-${lib.getExe pkgs.sccache}}"

          # Install whenever a lockfile is newer than the last `just install`, so
          # a `git pull` that bumps dependencies does not leave a stale tree. The
          # `.envrc` watches pnpm-lock.yaml and the whole nix/ tree, so direnv
          # re-enters the shell — and re-runs this check — as soon as one changes.
          # The tool dirs under nix/tools are outside the pnpm workspace, hence
          # the extra lockfiles.
          needsInstall=false
          if [ ! -e node_modules/.install-stamp ]; then
            needsInstall=true
          else
            for lock in pnpm-lock.yaml nix/tools/*/bun.lock; do
              if [ "$lock" -nt node_modules/.install-stamp ]; then
                needsInstall=true
              fi
            done
          fi
          if [ "$needsInstall" = true ]; then
            echo "Installing dependencies..."
            just install
          fi

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
