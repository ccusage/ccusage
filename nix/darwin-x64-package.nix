# Cross-compiled x86_64 (Intel) macOS build, produced on the aarch64 macOS
# runner so we no longer need a separate Intel macOS runner.
#
# This uses the native aarch64 toolchain with the x86_64-apple-darwin Rust
# target rather than a full pkgsCross stdenv: Apple clang is multi-arch (the
# `cc` crate auto-passes `-arch x86_64` for the C deps like bundled sqlite), and
# the Apple SDK ships multi-arch tbd stubs, so no cross stdenv bootstrap is
# needed.
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
    pkgs.lib.mkIf pkgs.stdenv.hostPlatform.isDarwin {
      packages.ccusage-darwin-x64 =
        let
          target = "x86_64-apple-darwin";
          crossCraneLib = (inputs.crane.mkLib pkgs).overrideToolchain (
            p:
            (p.rust-bin.fromRustupToolchainFile (root + /rust-toolchain.toml)).override {
              targets = [ target ];
            }
          );
          crossCommonArgs = config.packages.ccusage.passthru.commonArgs // {
            cargoExtraArgs = "-p ccusage --bin ccusage --target ${target}";
          };
          crossDepsOnlyArgs = config.packages.ccusage.passthru.depsOnlyArgs // {
            cargoExtraArgs = "-p ccusage --bin ccusage --target ${target}";
            # Clippy and hawk use the host layer, so this target only needs the
            # build output; Crane's default `cargo check` pass would add a
            # second, unused compile of every dependency.
            buildPhaseCargoCommand = "cargoWithProfile build -p ccusage --bin ccusage --target ${target}";
          };
          crossDependencyArtifacts = crossCraneLib.buildDepsOnly crossDepsOnlyArgs;
          crossWorkspaceArtifacts = import ./cargo-artifacts.nix {
            inherit root pkgs;
            craneLib = crossCraneLib;
            inherit (pkgs) lib;
            commonArgs = crossCommonArgs;
            cargoArtifacts = crossDependencyArtifacts;
            inherit (crossDepsOnlyArgs) dummySrc;
          };
          crossCargoArtifacts = crossWorkspaceArtifacts.adapters;
        in
        crossCraneLib.buildPackage (
          crossCommonArgs
          // {
            cargoArtifacts = crossCargoArtifacts;
            doInstallCheck = false;
            passthru = {
              cargoArtifacts = crossCargoArtifacts;
              dependencyArtifacts = crossDependencyArtifacts;
              workspaceArtifacts = crossWorkspaceArtifacts;
            };
            postInstall = config.packages.ccusage.passthru.systemDylibsGate;
            meta = config.packages.ccusage.meta // {
              description = "Intel (x86_64) macOS build of ccusage";
            };
          }
        );
    };
}
