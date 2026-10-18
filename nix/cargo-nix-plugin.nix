# Experimental per-crate build of ccusage via anthropics/cargo-nix-plugin, for
# comparing against the Crane layer chain. Evaluating it requires a Nix with the
# plugin loaded (`--option plugin-files <plugin>/lib/nix/plugins`), so it lives
# under legacyPackages where `nix flake show/check` never touch it.
{ inputs, ... }:
let
  root = ./..;
  rustRoot = root + /rust;
in
{
  perSystem =
    { system, ... }:
    let
      pkgs = import inputs.nixpkgs {
        inherit system;
        overlays = [ inputs.rust-overlay.overlays.default ];
      };
      inherit (pkgs) lib stdenv;
      rustToolchain = pkgs.rust-bin.fromRustupToolchainFile (root + /rust-toolchain.toml);
      inherit (lib.importJSON (root + /package.json)) version;
      cargoConfigEnv = (builtins.fromTOML (builtins.readFile (root + /.cargo/config.toml))).env;
      # Benchmark knob: a different salt forces every crate to rebuild without
      # re-downloading sources. Needs --impure to take effect.
      salt = builtins.getEnv "CNP_SALT";

      isWorkspaceCrate = name: lib.hasPrefix "ccusage" name;

      # Mirror [profile.release] + [profile.release.package."*"] from rust/Cargo.toml.
      profileOpts =
        isHost: args:
        if isHost || (args.procMacro or false) then
          [ ]
        else if args.crateName == "ccusage" then
          [
            "-C"
            "opt-level=z"
            "-C"
            "codegen-units=1"
            "-C"
            "panic=abort"
            "-C"
            "lto=fat"
            "-C"
            "strip=symbols"
          ]
          ++ lib.optionals stdenv.hostPlatform.isDarwin [
            "-C"
            "link-arg=-Wl,-dead_strip_dylibs"
            "-C"
            "link-arg=-fuse-ld=lld"
            "-C"
            "link-arg=-Wl,--icf=all"
          ]
          ++ lib.optionals stdenv.hostPlatform.isLinux [
            "-C"
            "link-arg=-Wl,--icf=all"
          ]
        else if isWorkspaceCrate args.crateName then
          [
            "-C"
            "opt-level=z"
            "-C"
            "codegen-units=1"
            "-C"
            "panic=abort"
          ]
        else
          [
            "-C"
            "opt-level=s"
            "-C"
            "codegen-units=1"
            "-C"
            "panic=abort"
          ];

      cargoNix = import "${inputs.cargo-nix-plugin}/lib" {
        inherit pkgs;
        src = rustRoot;
        # Narrow per-crate sources so editing one crate only rebuilds it and its
        # dependents instead of every workspace member.
        localSrc = relPath: {
          src = lib.fileset.toSource {
            root = rustRoot;
            fileset = lib.fileset.unions [
              (rustRoot + /Cargo.toml)
              (rustRoot + /Cargo.lock)
              (rustRoot + "/${relPath}")
            ];
          };
          workspace_member = relPath;
        };
        buildRustCrateForPkgs =
          _cratePkgs: base:
          {
            isHost ? false,
          }:
          args:
          base (
            args
            // {
              rust = rustToolchain;
              cargo = rustToolchain;
              extraRustcOpts = (args.extraRustcOpts or [ ]) ++ profileOpts isHost args;
            }
            // lib.optionalAttrs (salt != "") { CNP_SALT = salt; }
          );
        crateOverrides = pkgs.defaultCrateOverrides // {
          ccusage = _: {
            env.CCUSAGE_VERSION = version;
            nativeBuildInputs = lib.optionals stdenv.hostPlatform.isDarwin [ pkgs.lld ];
            buildInputs = lib.optionals stdenv.hostPlatform.isDarwin [ pkgs.apple-sdk_15 ];
          };
          ccusage-core = _: {
            env.CCUSAGE_PRICING_JSON_PATH = "${inputs.litellm}/model_prices_and_context_window.json";
          };
          sqlite3-src = _: { env = cargoConfigEnv; };
          # Cargo builds the vendored zstd statically; nixpkgs' default override
          # links the store dylib instead, which end-user Macs do not have.
          zstd-sys = _: { };
        };
      };
    in
    {
      legacyPackages.cnp = {
        inherit cargoNix;
        ccusage = cargoNix.workspaceMembers.ccusage.build;
        clippy = cargoNix.clippy.allWorkspaceMembers;
        # Toolchain and linkers both builders share, so a benchmark can fetch
        # them up front instead of charging them to whichever builder runs first.
        sharedInputs = pkgs.linkFarmFromDrvs "cnp-shared-inputs" (
          [ rustToolchain ]
          ++ lib.optionals stdenv.hostPlatform.isLinux [ pkgs.mold ]
          ++ lib.optionals stdenv.hostPlatform.isDarwin [
            pkgs.lld
            pkgs.apple-sdk_15
          ]
        );
      };
    };
}
