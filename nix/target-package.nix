{
  package,
  craneLib,
  pkgs,
  root,
  target,
  overrides ? { },
  doInstallCheck,
  postInstall,
  description,
  llvmProfdata ? null,
}:
let
  cargoExtraArgs = "-p ccusage --bin ccusage --target ${target}";
  commonArgs = package.passthru.commonArgs // overrides // { inherit cargoExtraArgs; };
  depsOnlyArgs =
    package.passthru.depsOnlyArgs
    // overrides
    // {
      inherit cargoExtraArgs;
      buildPhaseCargoCommand = "cargoWithProfile build ${cargoExtraArgs}";
    };
  dependencyArtifacts = craneLib.buildDepsOnly depsOnlyArgs;
  workspaceArtifacts = import ./cargo-artifacts.nix {
    inherit
      root
      pkgs
      craneLib
      commonArgs
      ;
    inherit (pkgs) lib;
    inherit (depsOnlyArgs) dummySrc;
    cargoArtifacts = dependencyArtifacts;
  };
  unsafeCodeCheck = import ./unsafe-code-check.nix {
    inherit
      commonArgs
      craneLib
      root
      target
      ;
    inherit (pkgs) lib nushell;
  };
  withRustflags = name: rustflags: {
    pname = "${commonArgs.pname}-${name}";
    RUSTFLAGS = "${commonArgs.RUSTFLAGS} ${rustflags}";
    cargoArtifacts = craneLib.buildDepsOnly (
      depsOnlyArgs
      // {
        pname = "${commonArgs.pname}-${name}";
        RUSTFLAGS = "${depsOnlyArgs.RUSTFLAGS} ${rustflags}";
      }
    );
  };
  instrumented = craneLib.buildPackage (
    commonArgs
    // withRustflags "pgo-instrumented" "-Cprofile-generate"
    // {
      doInstallCheck = false;
    }
  );
  pgoProfile = pkgs.callPackage ./pgo-profile.nix {
    inherit instrumented llvmProfdata root;
  };
  pgo = llvmProfdata != null;
in
craneLib.buildPackage (
  commonArgs
  // (
    if pgo then
      withRustflags "pgo" "-Cprofile-use=${pgoProfile}"
    else
      { cargoArtifacts = workspaceArtifacts.adapters; }
  )
  // {
    inherit (commonArgs) pname;
    inherit doInstallCheck postInstall;
    passthru = {
      inherit
        dependencyArtifacts
        unsafeCodeCheck
        workspaceArtifacts
        ;
      cargoArtifacts = workspaceArtifacts.adapters;
      # NOTE(2026-11-21, #2440): The instrumented dependencies depend only on Cargo.lock. Root them so the PGO build reuses them across commits.
      cacheRoot = pkgs.linkFarm "ccusage-release-cache-root" (
        [
          {
            name = "workspace";
            path = workspaceArtifacts.cacheRoot;
          }
        ]
        ++ pkgs.lib.optional pgo {
          name = "pgo-instrumented-dependencies";
          path = instrumented.cargoArtifacts;
        }
      );
    }
    // pkgs.lib.optionalAttrs pgo { inherit instrumented pgoProfile; };
    meta = package.meta // {
      inherit description;
    };
  }
)
