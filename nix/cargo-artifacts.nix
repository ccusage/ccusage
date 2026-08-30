# Crane artifact layers for the workspace, on top of the dependency-only cache.
#
# The layers are a linear chain — dependencies -> foundation -> adapters -> final
# binary — rather than one derivation per adapter. Each layer only extends the
# previous one, so Crane's full-archive install works everywhere, including
# Darwin, where it cannot install incremental archives
# (https://github.com/rust-lang/rust/issues/115982) and merging sibling archives
# would overwrite shared path-crate artifacts with incompatible variants.
#
# The adapters share one layer because they are mutually independent: a single
# `cargoBuild` with one `-p` per crate lets Cargo compile them concurrently, so
# the layer's wall clock is roughly the slowest single adapter. Splitting them
# into sibling derivations instead adds several serial Nix round-trips — sandbox
# setup, unpacking the dependency target directory, and writing a store archive
# per layer — which costs more than recompiling the unchanged adapters.
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
    "grok"
    "hermes"
    "kilo"
    "kimi"
    "openclaw"
    "opencode"
    "pi"
    "qwen"
    "zcode"
  ];
  adapterCrates = map (name: "ccusage-adapter-${name}") agentNames ++ [ "ccusage-adapter-all" ];

  # The adapters and the helpers they share live in adapters/<name>; every other
  # crate is a directory under crates/ named after itself.
  adapterDirNames = map (agent: "ccusage-adapter-${agent}") agentNames ++ [
    "ccusage-adapter-common"
  ];
  crateDir =
    name:
    if lib.elem name adapterDirNames then
      "adapters/${lib.removePrefix "ccusage-adapter-" name}"
    else
      "crates/${name}";
  crateSource = name: craneLib.fileset.commonCargoSources (rustRoot + "/${crateDir name}");
  extraSourcesFor =
    names:
    lib.optionals (lib.elem "ccusage-core" names) [
      (rustRoot + /crates/ccusage-core/src/fast-multiplier-overrides.json)
      (rustRoot + /crates/ccusage-core/src/models-dev-pricing.json)
      (rustRoot + /crates/ccusage-core/src/models-dev-catalog-rules.json)
    ]
    ++ lib.optionals (lib.elem "ccusage-adapter-codex" names) [
      (rustRoot + /adapters/codex/src/codex-auto-review-fallbacks.json)
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
  adapters = mkArtifacts {
    name = "ccusage-adapters";
    cargoArtifacts = foundation;
    packages = adapterCrates;
    sources = foundationCrates ++ adapterCrates;
  };
  cacheRoot = pkgs.linkFarm "ccusage-cargo-artifact-cache-root" [
    {
      name = "dependencies";
      path = cargoArtifacts;
    }
    {
      name = "foundation";
      path = foundation;
    }
    {
      name = "adapters";
      path = adapters;
    }
  ];
in
{
  inherit
    adapters
    cacheRoot
    foundation
    ;
}
