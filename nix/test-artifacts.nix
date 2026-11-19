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
      doCheck = false;
      buildPhaseCargoCommand = ''
        cargoWithProfile check --workspace --all-targets
        cargoWithProfile test --workspace --no-run
      '';
    }
  );
}
