{
  cargoArtifacts,
  commonArgs,
  craneLib,
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
    "ccusage-environment"
    "ccusage-adapter-common"
    "ccusage-terminal"
    "ccusage-zstd"
  ];
  workspace = builtins.fromTOML (builtins.readFile (rustRoot + /Cargo.toml));
  cratePaths = lib.mapAttrs (_: dependency: dependency.path) (
    lib.filterAttrs (_: dependency: dependency ? path) workspace.workspace.dependencies
  );
  adapterCrates = builtins.sort (left: right: cratePaths.${left} < cratePaths.${right}) (
    builtins.filter (name: lib.hasPrefix "ccusage-adapter-" name && name != "ccusage-adapter-common") (
      builtins.attrNames cratePaths
    )
  );
  crateSource =
    name:
    lib.fileset.intersection (lib.fileset.fromSource commonArgs.src) (
      rustRoot + "/${cratePaths.${name}}"
    );
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
      );
    };
  sourceFor =
    names:
    pkgs.runCommand "source" { } ''
      cp -R ${dummySrc} "$out"
      chmod -R u+w "$out"
      ${lib.concatMapStringsSep "\n" (name: ''rm -rf "$out/${cratePaths.${name}}"'') names}
      cp -R ${realSourceFor names}/. "$out"
    '';
  artifactCommonArgs =
    builtins.removeAttrs commonArgs [
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
