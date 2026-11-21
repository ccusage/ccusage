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
        ccusage-release-darwin-arm64 = darwinPackage "aarch64-apple-darwin" {
          doInstallCheck = true;
          llvmProfdata = "${rustToolchain}/lib/rustlib/aarch64-apple-darwin/bin/llvm-profdata";
          description = "Apple silicon macOS build of ccusage";
        };
        ccusage-release-darwin-x64 = config.packages.ccusage-darwin-x64;
        # NOTE(2026-06-11, #1289): Apple clang and the Apple SDK support both architectures. Use the native toolchain to cross-build the Intel binary.
        # NOTE(2026-11-21): PGO training runs the instrumented binary on the builder. This one is cross-compiled for Intel on Apple silicon, so it ships untrained.
        ccusage-darwin-x64 = darwinPackage "x86_64-apple-darwin" {
          doInstallCheck = false;
          description = "Intel (x86_64) macOS build of ccusage";
        };
      };
    };
}
