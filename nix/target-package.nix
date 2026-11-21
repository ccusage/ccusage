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
  maxMissingFunctions ? 0,
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
  withRustflags =
    name: rustflags:
    {
      final ? { },
      dependencies ? { },
    }:
    let
      args = {
        pname = "${commonArgs.pname}-${name}";
        RUSTFLAGS = "${commonArgs.RUSTFLAGS} ${rustflags}";
      };
    in
    args
    // final
    // {
      cargoArtifacts = craneLib.buildDepsOnly (depsOnlyArgs // args // dependencies);
    };
  instrumented = craneLib.buildPackage (
    commonArgs
    // withRustflags "pgo-instrumented" "-Cprofile-generate" { }
    // {
      doInstallCheck = false;
    }
  );
  pgoProfile = pkgs.callPackage ./pgo-profile.nix {
    inherit instrumented llvmProfdata root;
  };
  keepRustcDiagnostics = pkgs.writeShellScript "rustc-keep-diagnostics" ''
    log=$(mktemp)
    status=0
    "$@" 2>"$log" || status=$?
    cat "$log" >&2
    cat "$log" >> "$NIX_BUILD_TOP/rustc-diagnostics.log"
    rm -f "$log"
    exit "$status"
  '';
  # NOTE(2026-11-21, #2457): LLVM only warns when it drops profile data, and -D warnings does not cover backend diagnostics. Fail on them so a toolchain bump cannot silently ship an untrained binary.
  rejectIgnoredProfile =
    {
      ignoredCrates ? "^$",
      maxMissingFunctions ? 0,
    }:
    {
      RUSTC_WRAPPER = keepRustcDiagnostics;
      postBuild = ''
        touch "$NIX_BUILD_TOP/rustc-diagnostics.log"
        ignored=$(grep -o '[a-z0-9_]*\.[0-9a-f]*-cgu\.[0-9]*: \(function control flow change detected (hash mismatch)\|no profile data available for function\) [^ "\\]*' \
          "$NIX_BUILD_TOP/rustc-diagnostics.log" | grep -Ev '${ignoredCrates}' | sort -u || true)
        mismatched=$(grep -c 'hash mismatch' <<< "$ignored" || true)
        missing=$(grep -c 'no profile data' <<< "$ignored" || true)
        echo "PGO profile: $mismatched mismatched and $missing missing functions (at most ${toString maxMissingFunctions} missing allowed)"
        if [ "$mismatched" -gt 0 ] || [ "$missing" -gt ${toString maxMissingFunctions} ]; then
          echo "error: rustc ignored the PGO profile for these functions:" >&2
          grep 'hash mismatch' <<< "$ignored" | head -n 10 >&2 || true
          grep 'no profile data' <<< "$ignored" | head -n 10 >&2 || true
          exit 1
        fi
      '';
    };
  pgo = llvmProfdata != null;
  finalArgs =
    if pgo then
      withRustflags "pgo" "-Cprofile-use=${pgoProfile} -Cllvm-args=-pgo-warn-missing-function" {
        final = rejectIgnoredProfile { inherit maxMissingFunctions; };
        # NOTE(2026-11-21, #2457): Crane's dependency layer compiles stub workspace crates. They never ran in training, so their functions have no profile.
        dependencies = rejectIgnoredProfile { ignoredCrates = "^ccusage(_[a-z0-9_]+)?\\."; };
      }
    else
      { cargoArtifacts = workspaceArtifacts.adapters; };
in
craneLib.buildPackage (
  commonArgs
  // finalArgs
  // {
    inherit (commonArgs) pname;
    inherit doInstallCheck postInstall;
    passthru = {
      inherit
        dependencyArtifacts
        unsafeCodeCheck
        workspaceArtifacts
        ;
      inherit (finalArgs) cargoArtifacts;
      # NOTE(2026-11-21, #2440): The instrumented dependencies depend only on Cargo.lock. They are the one layer a PGO build reuses, because the profile-use layers change with every profile.
      cacheRoot =
        if pgo then
          pkgs.linkFarm "ccusage-pgo-cache-root" [
            {
              name = "instrumented-dependencies";
              path = instrumented.cargoArtifacts;
            }
          ]
        else
          workspaceArtifacts.cacheRoot;
    }
    // pkgs.lib.optionalAttrs pgo { inherit instrumented pgoProfile; };
    meta = package.meta // {
      inherit description;
    };
  }
)
