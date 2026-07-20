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
    in
    {
      devShells.default = pkgs.mkShell {
        buildInputs =
          (with pkgs; [
            nodejs
            pnpm
            nushell
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
          ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
            pkgs.mold
          ]
          ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
            pkgs.apple-sdk_15
          ]
          ++ config.pre-commit.settings.enabledPackages;

        shellHook = ''
          if [ "$(uname -s)" = "Linux" ]; then
            case " ''${RUSTFLAGS:-} " in
              *" -C link-arg=-fuse-ld=mold "*) ;;
              *) export RUSTFLAGS="''${RUSTFLAGS:+$RUSTFLAGS }-C link-arg=-fuse-ld=mold" ;;
            esac
          fi
          ${lib.getExe config.packages.syncAgentSkills}
          ${config.pre-commit.shellHook}

          # Set up direnv and JS dependencies whenever `git wt` creates a new
          # worktree, so worktrees are usable without a manual step.
          git config --replace-all wt.hook "direnv allow || true; pnpm install --frozen-lockfile || true"

          # Move deleted worktree directories to the trash instead of `rm -rf`,
          # which is noticeably slower on large node_modules and target trees.
          git config --replace-all wt.remover "${pkgs.trash-cli}/bin/trash"

          # Free the worktree's direnv/nix state before it's deleted.
          git config --replace-all wt.deletehook "direnv revoke .envrc || true; ${pkgs.trash-cli}/bin/trash .direnv || true; nix store gc || true"
        '';
      };
    };
}
