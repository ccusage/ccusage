# Cross-compiled x86_64 (Intel) macOS build, produced on the aarch64 macOS
# runner so we no longer need a separate Intel macOS runner.
#
# This uses the native aarch64 toolchain with the x86_64-apple-darwin Rust
# target rather than a full pkgsCross stdenv: Apple clang is multi-arch (the
# `cc` crate auto-passes `-arch x86_64` for the C deps like bundled sqlite), and
# the Apple SDK ships multi-arch tbd stubs, so no cross stdenv bootstrap is
# needed. The nixpkgs libiconv (aarch64-only) is dropped; the binary links the
# SDK's multi-arch libSystem and nothing else, so it is portable to end-user
# Intel Macs (verified: `otool -L` shows only /usr/lib/libSystem.B.dylib).
{
  inputs,
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
    in
    pkgs.lib.mkIf pkgs.stdenv.isDarwin {
      packages.ccusage-darwin-x64 =
        let
          target = "x86_64-apple-darwin";
          crossCraneLib = (inputs.crane.mkLib pkgs).overrideToolchain (
            p:
            (p.rust-bin.fromRustupToolchainFile (root + /rust-toolchain.toml)).override {
              targets = [ target ];
            }
          );
          # Inherit Darwin buildInputs from the base package and only filter out
          # the nixpkgs libiconv (aarch64-only); the binary links the multi-arch
          # libSystem from the SDK instead. Filtering rather than hardcoding the
          # list keeps future Darwin deps in the base package from being dropped
          # silently.
          crossBuildInputs = pkgs.lib.filter (
            dep: dep != pkgs.libiconv
          ) config.packages.ccusage.passthru.commonArgs.buildInputs;
          crossCommonArgs = config.packages.ccusage.passthru.commonArgs // {
            cargoExtraArgs = "-p ccusage --bin ccusage --target ${target}";
            buildInputs = crossBuildInputs;
          };
          crossDepsOnlyArgs = config.packages.ccusage.passthru.depsOnlyArgs // {
            cargoExtraArgs = "-p ccusage --bin ccusage --target ${target}";
            buildInputs = crossBuildInputs;
          };
          crossDependencyArtifacts = crossCraneLib.buildDepsOnly crossDepsOnlyArgs;
          crossWorkspaceArtifacts = import ./cargo-artifacts.nix {
            inherit root pkgs;
            craneLib = crossCraneLib;
            inherit (pkgs) lib;
            commonArgs = crossCommonArgs;
            cargoArtifacts = crossDependencyArtifacts;
            cargoTargetArgs = "--target ${target}";
          };
          crossCargoArtifacts = crossWorkspaceArtifacts.adapters;
        in
        crossCraneLib.buildPackage (
          crossCommonArgs
          // {
            cargoArtifacts = crossCargoArtifacts;
            passthru = {
              cargoArtifacts = crossCargoArtifacts;
              dependencyArtifacts = crossDependencyArtifacts;
              workspaceArtifacts = crossWorkspaceArtifacts;
            };
            # End-user Intel Macs have no /nix/store, so fail the build if the
            # binary links anything outside the macOS system paths.
            postInstall = ''
              for lib in $(otool -L "$out/bin/ccusage" | tail -n +2 | awk '{print $1}' | grep -E '^/nix/store/[^/]+-libiconv-'); do
                install_name_tool -change "$lib" /usr/lib/libiconv.2.dylib "$out/bin/ccusage"
              done
              if otool -L "$out/bin/ccusage" | tail -n +2 | awk '{print $1}' | grep -Ev '^(/usr/lib/|/System/Library/)'; then
                echo "error: ccusage-darwin-x64 links dylibs that do not exist on end-user machines" >&2
                exit 1
              fi
            '';
            meta = config.packages.ccusage.meta // {
              description = "Intel (x86_64) macOS build of ccusage";
            };
          }
        );
    };
}
