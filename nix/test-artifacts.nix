# Dependency layer for everything that is not the release binary: the test run,
# clippy and hawk.
#
# The release `cargoArtifacts` cannot serve them. Tests and clippy compile the
# whole workspace with `--all-targets`, which pulls in dev-dependencies, and the
# `test-snapshots` feature variants of the workspace crates, neither of which
# the release layers contain; hawk runs `cargo check`, whose metadata-only
# artifacts a release `cargo build` never produces. Without this layer each of
# them recompiled all of that on every CI run.
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
  depsOnlyArgs,
  cargoHawk,
}:
let
  profileEnv = {
    CARGO_PROFILE = "dev";
  };
in
{
  # Args every consumer of the layer (tests, clippy, hawk) must share. None of
  # them feeds a later derivation, so archiving their ~1 GiB target directory
  # would only add compression time.
  consumerArgs = profileEnv // {
    doInstallCargoArtifacts = false;
  };
  # hawk drives cargo through its own instrumented compiler wrapper, so its
  # dependency units never match the ones above; it gets a layer of its own,
  # built with the same invocation over the dummy sources.
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
      doCheck = true;
      buildPhaseCargoCommand = ''
        cargoWithProfile check --workspace --all-targets
        cargoWithProfile test --workspace --no-run
      '';
    }
  );
}
