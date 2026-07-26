{
  cargoArtifacts,
  cargoTargetArgs ? "",
  commonArgs,
  craneLib,
  lib,
  pkgs,
  root,
}:
let
  rustRoot = root + /rust;
  foundationCrates = [
    "ccusage-cli"
    "ccusage-core"
    "ccusage-adapter-common"
    "ccusage-terminal"
    "ccusage-test-support"
  ];
  agentNames = [
    "amp"
    "claude"
    "codebuff"
    "codex"
    "copilot"
    "droid"
    "gemini"
    "goose"
    "hermes"
    "kilo"
    "kimi"
    "openclaw"
    "opencode"
    "pi"
    "qwen"
  ];
  adapterCrate = name: "ccusage-adapter-${name}";
  allAdapterCrates = map adapterCrate agentNames;

  crateSource = name: craneLib.fileset.commonCargoSources (rustRoot + "/crates/${name}");
  extraSourcesFor =
    names:
    lib.optionals (lib.elem "ccusage-cli" names) [
      (rustRoot + /crates/ccusage-cli/src/cli-help.json)
      (rustRoot + /crates/ccusage-cli/src/cli-commands.json)
    ]
    ++ lib.optionals (lib.elem "ccusage-core" names) [
      (rustRoot + /crates/ccusage-core/src/fast-multiplier-overrides.json)
      (rustRoot + /crates/ccusage-core/src/models-dev-pricing.json)
    ]
    ++ lib.optionals (lib.elem "ccusage-adapter-codex" names) [
      (rustRoot + /crates/ccusage-adapter-codex/src/codex-auto-review-fallbacks.json)
    ];
  sourceFor =
    names:
    lib.fileset.toSource {
      root = rustRoot;
      fileset = lib.fileset.unions (
        [
          (rustRoot + /Cargo.toml)
          (rustRoot + /Cargo.lock)
        ]
        ++ map crateSource names
        ++ extraSourcesFor names
      );
    };
  packageArgs =
    names:
    lib.concatStringsSep " " (map (name: "-p ${name}") names)
    + lib.optionalString (cargoTargetArgs != "") " ${cargoTargetArgs}";
  artifactCommonArgs =
    builtins.removeAttrs commonArgs [
      "CCUSAGE_VERSION"
      "cargoExtraArgs"
      "src"
    ]
    // {
      version = "0.0.0";
      doCheck = false;
      doInstallCargoArtifacts = true;
    };
  mkArtifacts =
    {
      cargoArtifacts,
      name,
      packages,
      sources,
    }:
    craneLib.cargoBuild (
      artifactCommonArgs
      // {
        pname = "${name}-artifacts";
        inherit cargoArtifacts;
        src = sourceFor sources;
        cargoExtraArgs = packageArgs packages;
      }
    );

  foundation = mkArtifacts {
    name = "ccusage-foundation";
    inherit cargoArtifacts;
    packages = foundationCrates;
    sources = foundationCrates;
  };
  opencode = mkArtifacts {
    name = "ccusage-adapter-opencode";
    cargoArtifacts = foundation;
    packages = [ "ccusage-adapter-opencode" ];
    sources = foundationCrates ++ [ "ccusage-adapter-opencode" ];
  };
  amp = mkArtifacts {
    name = "ccusage-adapter-amp";
    cargoArtifacts = foundation;
    packages = [ "ccusage-adapter-amp" ];
    sources = foundationCrates ++ [ "ccusage-adapter-amp" ];
  };
  mkFoundationAdapter =
    name:
    mkArtifacts {
      name = "ccusage-adapter-${name}";
      cargoArtifacts = foundation;
      packages = [ (adapterCrate name) ];
      sources = foundationCrates ++ [ (adapterCrate name) ];
    };
  adapterArtifacts = {
    inherit amp opencode;
    claude = mkFoundationAdapter "claude";
    codex = mkFoundationAdapter "codex";
    codebuff = mkFoundationAdapter "codebuff";
    goose = mkFoundationAdapter "goose";
    copilot = mkFoundationAdapter "copilot";
    droid = mkFoundationAdapter "droid";
    gemini = mkFoundationAdapter "gemini";
    hermes = mkFoundationAdapter "hermes";
    kilo = mkFoundationAdapter "kilo";
    kimi = mkFoundationAdapter "kimi";
    openclaw = mkFoundationAdapter "openclaw";
    pi = mkFoundationAdapter "pi";
    qwen = mkFoundationAdapter "qwen";
  };
  merged =
    pkgs.runCommand "ccusage-adapter-artifacts-merged"
      {
        nativeBuildInputs = [
          craneLib.inheritCargoArtifactsHook
          craneLib.installCargoArtifactsHook
          pkgs.coreutils
          pkgs.findutils
          pkgs.gnutar
          pkgs.rsync
          pkgs.zstd
        ];
      }
      ''
        export CARGO_TARGET_DIR="$TMPDIR/target"
        export doCompressAndInstallFullArchive=1
        mkdir -p "$CARGO_TARGET_DIR"
        inheritCargoArtifacts ${foundation}
        inheritCargoArtifactDelta() {
          echo "decompressing cargo artifact delta from $1"
          zstd -d "$1/target.tar.zst" --stdout | tar -x -C "$CARGO_TARGET_DIR"
        }
        ${lib.concatMapStringsSep "\n" (artifacts: "inheritCargoArtifactDelta ${artifacts}") (
          lib.attrValues adapterArtifacts
        )}
        prepareAndInstallCargoArtifactsDir "$out" "$CARGO_TARGET_DIR" "use-zstd" ""
      '';
  all = mkArtifacts {
    name = "ccusage-adapter-all";
    cargoArtifacts = merged;
    packages = [ "ccusage-adapter-all" ];
    sources = foundationCrates ++ allAdapterCrates ++ [ "ccusage-adapter-all" ];
  };
  cacheRoot = pkgs.linkFarm "ccusage-cargo-artifact-cache-root" (
    [
      {
        name = "dependencies";
        path = cargoArtifacts;
      }
      {
        name = "foundation";
        path = foundation;
      }
      {
        name = "merged";
        path = merged;
      }
      {
        name = "all";
        path = all;
      }
    ]
    ++ lib.mapAttrsToList (name: path: {
      name = "adapter-${name}";
      inherit path;
    }) adapterArtifacts
  );
in
{
  inherit
    adapterArtifacts
    all
    cacheRoot
    foundation
    merged
    ;
}
