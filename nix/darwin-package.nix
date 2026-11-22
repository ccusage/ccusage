{ inputs, lib, ... }:
{
  perSystem =
    {
      config,
      pkgs,
      rustToolchain,
      ...
    }:
    let
      darwinPackage =
        target: args:
        import ./target-package.nix (
          {
            package = config.packages.ccusage;
            inherit pkgs target;
            root = ./..;
            craneLib = (inputs.crane.mkLib pkgs).overrideToolchain (
              p: (p.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml).override { targets = [ target ]; }
            );
            postInstall = config.packages.ccusage.passthru.systemDylibsGate;
          }
          // args
        );
    in
    lib.mkIf pkgs.stdenv.hostPlatform.isDarwin {
      packages = {
        ccusage-release-darwin-arm64 =
          (darwinPackage "aarch64-apple-darwin" {
            doInstallCheck = true;
            llvmProfdata = "${rustToolchain}/lib/rustlib/aarch64-apple-darwin/bin/llvm-profdata";
            # NOTE(2026-11-21, #2457): A fresh profile still lacks 6,683 generic instantiations that only the Apple silicon profile-use build creates, with no hash mismatch. A profile that stopped matching leaves nearly every function missing.
            maxMissingFunctions = 10000;
            description = "Apple silicon macOS build of ccusage";
          }).overrideAttrs
            (_: {
              CCUSAGE_TELEMETRY_ENDPOINT = "https://telemetry.ccusage.com/v1/ping";
            });
        ccusage-release-darwin-x64 = config.packages.ccusage-darwin-x64.overrideAttrs (_: {
          CCUSAGE_TELEMETRY_ENDPOINT = "https://telemetry.ccusage.com/v1/ping";
        });
        # NOTE(2026-06-11, #1289): Apple clang and the Apple SDK support both architectures. Use the native toolchain to cross-build the Intel binary.
        # NOTE(2026-11-21): PGO training runs the instrumented binary on the builder. This one is cross-compiled for Intel on Apple silicon, so it ships untrained.
        ccusage-darwin-x64 = darwinPackage "x86_64-apple-darwin" {
          doInstallCheck = false;
          description = "Intel (x86_64) macOS build of ccusage";
        };
      };
    };
}
