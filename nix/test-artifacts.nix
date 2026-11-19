# Dependency layer for everything that is not the release binary: the test run
# and clippy.
#
# The release `cargoArtifacts` cannot serve them. Tests and clippy compile the
# whole workspace with `--all-targets`, which pulls in dev-dependencies, and the
# dev-only feature variants of the workspace crates (`test-clock`,
# `testing`), neither of which the release layers contain. Without this layer each of them recompiled all of that on every CI run.
#
# It uses the dev profile on purpose: these consumers never ship a binary, so
# fat LTO and size optimization would only slow the compile. Every consumer
# must use the same profile, features (`--workspace`) and RUSTFLAGS as this
# layer, or Cargo's fingerprints no longer match and it rebuilds the
# dependencies anyway.
#
# The layer runs both `cargo check` and `cargo test --no-run`: clippy reuses
# the former, the test binaries the latter, and the two sets of artifacts are
# independent.
{
  craneLib,
  cargoHawk,
  depsOnlyArgs,
}:
let
  profileEnv = {
    CARGO_PROFILE = "dev";
  };
in
{
  # Args every consumer of the layer (tests, clippy) must share. None of
  # them feeds a later derivation, so archiving their ~1 GiB target directory
  # would only add compression time.
  consumerArgs = profileEnv // {
    doInstallCargoArtifacts = false;
  };
  hawkArtifacts = craneLib.buildDepsOnly (
    depsOnlyArgs
    // profileEnv
    // {
      pnameSuffix = "-hawk-deps";
      doCheck = false;
      nativeBuildInputs = depsOnlyArgs.nativeBuildInputs or [ ] ++ [ cargoHawk ];
      buildPhaseCargoCommand = ''
        cargo hawk check --manifest-path Cargo.toml --target-dir "''${CARGO_TARGET_DIR:-target}"
      '';
    }
  );
  cargoArtifacts = craneLib.buildDepsOnly (
    depsOnlyArgs
    // profileEnv
    // {
      pnameSuffix = "-test-deps";
      # The build command already compiles the test binaries; crane's check
      # phase would only add another `cargo test --no-run` for the ccusage bin.
      doCheck = false;
      buildPhaseCargoCommand = ''
        cargoWithProfile check --workspace --all-targets
        cargoWithProfile test --workspace --no-run
      '';
    }
  );
}
