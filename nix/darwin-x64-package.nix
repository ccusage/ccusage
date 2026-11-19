{ inputs, lib, ... }:
{
  perSystem =
    { config, pkgs, ... }:
    lib.mkIf pkgs.stdenv.hostPlatform.isDarwin {
      packages = {
        ccusage-release-darwin-arm64 = config.packages.ccusage;
        ccusage-release-darwin-x64 = config.packages.ccusage-darwin-x64;
        ccusage-darwin-x64 = import ./target-package.nix {
          # NOTE(2026-06-11, #1289): Apple clang and the Apple SDK support both architectures. Use the native toolchain to cross-build the Intel binary.
          package = config.packages.ccusage;
          inherit pkgs;
          root = ./..;
          target = "x86_64-apple-darwin";
          craneLib = (inputs.crane.mkLib pkgs).overrideToolchain (
            p:
            (p.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml).override {
              targets = [ "x86_64-apple-darwin" ];
            }
          );
          doInstallCheck = false;
          postInstall = config.packages.ccusage.passthru.systemDylibsGate;
          description = "Intel (x86_64) macOS build of ccusage";
        };
      };
    };
}
