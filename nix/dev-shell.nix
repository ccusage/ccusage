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
        CCUSAGE_PRICING_JSON_PATH = "${inputs.litellm}/model_prices_and_context_window.json";

        buildInputs =
          (with pkgs; [
            nodejs
            pnpm
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
        ''
        + gitWtHook;
      };
    };
}
