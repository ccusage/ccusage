{ inputs, lib, ... }:
{
  perSystem =
    { config, pkgs, ... }:
    lib.mkIf pkgs.stdenv.hostPlatform.isDarwin {
      packages = {
        ccusage-release-darwin-arm64 = config.packages.ccusage;
        ccusage-release-darwin-x64 = config.packages.ccusage-darwin-x64;
        ccusage-darwin-x64 = import ./target-package.nix {
          package = config.packages.ccusage;
          inherit pkgs;
          root = ./..;
          target = "x86_64-apple-darwin";
          # NOTE(2026-06-11, #1289): the native toolchain cross-builds Intel without
          # a pkgsCross stdenv because Apple clang is multi-arch (the cc crate passes
          # `-arch x86_64` for bundled C deps) and the Apple SDK ships multi-arch tbd
          # stubs.
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
