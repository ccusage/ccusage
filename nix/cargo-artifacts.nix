# Crane artifact layers for the workspace, on top of the dependency-only cache.
#
# The layers are a linear chain — dependencies -> foundation -> adapters -> final
# binary — rather than one derivation per adapter. Each layer only extends the
# previous one, so Crane's full-archive install works everywhere, including
# Darwin, where it cannot install incremental archives
# (https://github.com/rust-lang/rust/issues/115982) and merging sibling archives
# would overwrite shared path-crate artifacts with incompatible variants.
#
# Every layer runs the final package's own build command (`-p ccusage --bin
# ccusage`, plus the target for the static and cross builds) over a source tree
# where only that layer's crates are real and every other workspace crate keeps
# Crane's dummy source. Cargo unifies dependency features over the packages it
# was asked to build, so building a subset with `-p` resolves different features
# for shared dependencies (serde_json's float_roundtrip, which only the pricing
# snapshot crate enables, for one) and hands the layer's crates different
# fingerprints from the final build's, which then recompiles all of them. With
# the same command the units match, and the final layer only compiles the
# binary crate and the leaf crates kept out of the layers (the pricing
# snapshots, the CLI parser and the config schema). Linking the dummy binary is
# cheap: its `main` reaches nothing, so fat LTO discards the rest.
#
# Moving a crate from dummy to real is detected by its fingerprint, not by
# timestamps (every file in the sandbox carries the store's epoch mtime): Crane
# points each dummy manifest at a dummy build script, so the real crate's unit
# has different dependencies.
#
# The adapters share one layer because they are mutually independent: a single
# build lets Cargo compile them concurrently, so the layer's wall clock is
# roughly the slowest single adapter. Splitting them into sibling derivations
# instead adds several serial Nix round-trips — sandbox setup, unpacking the
# dependency target directory, and writing a store archive per layer — which
# costs more than recompiling the unchanged adapters.
{
  cargoArtifacts,
  commonArgs,
  craneLib,
  # The dependency layer's dummy source; the layers overlay their real crates on it.
  dummySrc,
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
  ];
  agentNames = [
    "amp"
    "antigravity"
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
    "muse"
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
      (rustRoot + /crates/ccusage-core/src/flex-multiplier-overrides.json)
    ]
    ++ lib.optionals (lib.elem "ccusage-adapter-codex" names) [
      (rustRoot + /adapters/codex/src/codex-auto-review-fallbacks.json)
    ];
  realSourceFor =
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
  # Named `source` like the final package's source, so every layer unpacks to the
  # same directory. The real root manifest and lockfile replace Crane's cleaned
  # copies, so the real crates resolve against what the final build reads.
  sourceFor =
    names:
    pkgs.runCommand "source" { } ''
      cp -R ${dummySrc} "$out"
      chmod -R u+w "$out"
      ${lib.concatMapStringsSep "\n" (name: ''rm -rf "$out/${crateDir name}"'') names}
      cp -R ${realSourceFor names}/. "$out"
    '';
  artifactCommonArgs =
    builtins.removeAttrs commonArgs [
      # Only the snapshot crate's build script reads it, and it stays a dummy here.
      "CCUSAGE_PRICING_JSON_PATH"
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
      realCrates,
    }:
    craneLib.mkCargoDerivation (
      artifactCommonArgs
      // {
        pname = "${name}-artifacts";
        pnameSuffix = "-build";
        inherit cargoArtifacts;
        src = sourceFor realCrates;
        buildPhaseCargoCommand = "cargoWithProfile build ${commonArgs.cargoExtraArgs}";
      }
    );

  foundation = mkArtifacts {
    name = "ccusage-foundation";
    inherit cargoArtifacts;
    realCrates = foundationCrates;
  };
  adapters = mkArtifacts {
    name = "ccusage-adapters";
    cargoArtifacts = foundation;
    realCrates = foundationCrates ++ adapterCrates;
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
