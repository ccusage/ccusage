{ inputs, lib, ... }:
{
  perSystem =
    {
      config,
      pkgs,
      rustToolchain,
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
      staticPackage =
        args:
        import ./target-package.nix (
          {
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
            postInstall = ''
              if "''${READELF:-readelf}" -l $out/bin/ccusage | grep -q INTERP; then
                echo "error: ccusage-static is not statically linked" >&2
                exit 1
              fi
            '';
            description = "Static Linux build of ccusage";
          }
          // args
        );
    in
    lib.mkIf pkgs.stdenv.hostPlatform.isLinux {
      packages = {
        "ccusage-release-linux-${arch}" = config.packages.ccusage-static;
        "ccusage-release-linux-${arch}-telemetry" = config.packages.ccusage-static-telemetry;
        # NOTE(2026-06-11, #1279): Blacksmith trims the sticky disk to Nix GC roots. Expose the static build artifacts so CI can root them across runs.
        ccusage-static = staticPackage {
          llvmProfdata = "${rustToolchain}/lib/rustlib/${pkgs.stdenv.hostPlatform.rust.rustcTarget}/bin/llvm-profdata";
        };
        ccusage-static-telemetry = staticPackage {
          llvmProfdata = "${rustToolchain}/lib/rustlib/${pkgs.stdenv.hostPlatform.rust.rustcTarget}/bin/llvm-profdata";
          telemetryEnabled = true;
        };
        # NOTE(2026-11-21, #2457): PGO profiles vary between training runs. The PR perf comparison builds this untrained twin so it measures code changes, not profile noise.
        ccusage-static-no-pgo = staticPackage {
          description = "Static Linux build of ccusage without PGO";
          # NOTE(2026-11-22, #2514): Without PGO, relinking the same source moved `opencode daily` by up to 14% depending on where hot functions fell within 64-byte lines. Starting every function on a 64-byte line fixes that placement, so unrelated code changes no longer shift it.
          extraRustflags = "-C llvm-args=-align-all-functions=6";
        };
      };
    };
}
