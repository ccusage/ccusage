{
  craneLib,
  commonArgs,
  lib,
  nushell,
  root,
  target ? null,
  cargoArtifacts ? null,
  withScriptTests ? false,
}:
let
  scripts = lib.fileset.toSource {
    root = root + /scripts;
    fileset = lib.fileset.unions [
      (root + /scripts/check-unsafe-code.nu)
      (root + /scripts/check-unsafe-code.test.nu)
      (root + /scripts/forbid-unsafe-rustc.rs)
    ];
  };
  targetArgs = lib.optionalString (target != null) "--target ${target}";
in
craneLib.mkCargoDerivation (
  builtins.removeAttrs commonArgs [
    "cargoExtraArgs"
    "installCheckPhase"
  ]
  // {
    pname = "ccusage-unsafe-code" + lib.optionalString (target != null) "-${target}";
    src = import ./rust-src.nix { inherit lib root; };
    sourceRoot = "source/rust";
    cargoLock = root + /rust/Cargo.lock;
    inherit cargoArtifacts;
    doInstallCargoArtifacts = false;
    nativeBuildInputs = commonArgs.nativeBuildInputs or [ ] ++ [ nushell ];
    buildPhaseCargoCommand = ''
      ${lib.optionalString withScriptTests "nu --no-config-file ${scripts}/check-unsafe-code.test.nu"}
      nu --no-config-file ${scripts}/check-unsafe-code.nu Cargo.toml --target-dir "''${CARGO_TARGET_DIR:-target}" ${targetArgs}
    '';
    installPhaseCommand = "touch $out";
  }
)
