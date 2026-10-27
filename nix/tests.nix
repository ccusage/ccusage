# Rust workspace test run as a Nix derivation so it shares the crane
# `cargoArtifacts` cache (warm on the per-job Blacksmith sticky disk) instead of
# recompiling into an uncached `rust/target` on every CI run. Built by the CI
# test job via `nix build .#ccusage-tests`; the derivation succeeds only when
# `cargo test --workspace` passes. It runs in the dev profile on a dedicated
# dependency layer (nix/test-artifacts.nix) rather than recompiling in release.
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
      inherit (config.packages.ccusage.passthru) commonArgs depsOnlyArgs;
      testArtifacts = import ./test-artifacts.nix {
        inherit craneLib depsOnlyArgs;
        cargoHawk = config.packages.cargo-hawk;
      };
      testSrc = import ./rust-src.nix { inherit lib root; };
    in
    {
      packages.ccusage-tests = craneLib.cargoTest (
        commonArgs
        // testArtifacts.consumerArgs
        // {
          src = testSrc;
          sourceRoot = "source/rust";
          cargoLock = root + /rust/Cargo.lock;
          # Dev-profile layer holding dev-dependencies and the test feature
          # set; the release layers would leave all of that to compile here.
          inherit (testArtifacts) cargoArtifacts;
          # Exposed so CI can root the layer on the sticky disk, like the
          # release layers, instead of letting GC drop it between runs.
          passthru = { inherit (testArtifacts) cargoArtifacts hawkArtifacts; };
          # commonArgs disables checks for the release build; re-enable here so
          # cargoTest actually runs the test suite rather than skipping it.
          doCheck = true;
          cargoExtraArgs = "--workspace";
          # jiff resolves named time zones (e.g. Asia/Tokyo) from the system
          # zoneinfo database, which the hermetic build sandbox lacks, so it
          # would fall back to UTC and shift the timezone-dependent tests by a
          # day. Point it at the nixpkgs tzdata; referencing the store path here
          # also pulls it into the sandbox as a build input.
          TZDIR = "${pkgs.tzdata}/share/zoneinfo";
          # The workspace tests resolve default Claude data directories from
          # $HOME; the sandbox has none, so seed a writable HOME with the same
          # empty directory layout the CI test job creates before running tests.
          preBuild = ''
            export HOME=$(mktemp -d)
            mkdir -p "$HOME/.claude/projects" "$HOME/.config/claude/projects"
          '';
        }
      );
    };
}
