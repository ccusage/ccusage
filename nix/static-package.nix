{ inputs, lib, ... }:
{
  perSystem =
    {
      config,
      pkgs,
      system,
      ...
    }:
    let
      target =
        if system == "x86_64-linux" then "x86_64-unknown-linux-musl" else "aarch64-unknown-linux-musl";
      arch = if system == "x86_64-linux" then "x64" else "arm64";
      staticPkgs =
        if system == "x86_64-linux" then
          pkgs.pkgsCross.musl64
        else
          pkgs.pkgsCross.aarch64-multiplatform-musl;
    in
    lib.mkIf pkgs.stdenv.hostPlatform.isLinux {
      packages = {
        # NOTE(2026-06-11, #1260): on Linux the default `.#ccusage` is
        # glibc-dynamic with a runpath into /nix/store, which end-user machines
        # cannot load, so Linux release artifacts must come from this static
        # musl build.
        "ccusage-release-linux-${arch}" = config.packages.ccusage-static;
        ccusage-static = import ./target-package.nix {
          package = config.packages.ccusage;
          inherit pkgs target;
          root = ./..;
          craneLib = (inputs.crane.mkLib staticPkgs).overrideToolchain (
            p: (p.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml).override { targets = [ target ]; }
          );
          overrides = {
            nativeBuildInputs = with staticPkgs; [
              mold
              pkg-config
            ];
            buildInputs = [ ];
          };
          doInstallCheck = true;
          # NOTE(2026-06-10, #1256): a PT_INTERP header requests a dynamic loader
          # that end-user machines lack. The cross bintools wrapper exports READELF.
          postInstall = ''
            if "''${READELF:-readelf}" -l $out/bin/ccusage | grep -q INTERP; then
              echo "error: ccusage-static is not statically linked" >&2
              exit 1
            fi
          '';
          description = "Static Linux build of ccusage";
        };
      };
    };
}
