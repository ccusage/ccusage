{
  craneLib,
  inputs,
  lib,
  lld,
  mold,
  nushell,
  pkgs,
  pkg-config,
  root ? ./.,
  stdenv,
  apple-sdk_15,
  telemetryEndpoint ? null,
}:
let
  inherit (lib.importJSON (root + /package.json)) version;
  cliPackageJson = lib.importJSON (root + /apps/ccusage/package.json);
  rustRoot = root + /rust;
  src = lib.cleanSourceWith {
    src = rustRoot;
    filter =
      path: type:
      let
        relativePath = lib.removePrefix "${toString rustRoot}/" (toString path);
      in
      craneLib.filterCargoSources path type
      || builtins.match "(adapters|crates)/[^/]+/src/.*" relativePath != null;
  };
  cargoConfigEnv = (builtins.fromTOML (builtins.readFile (root + /.cargo/config.toml))).env;
  # NOTE(2026-07-27, #1428): Apple ld64 lacks identical-code folding. Link with lld on Darwin to fold duplicate adapter instantiations.
  linkerRustflags =
    lib.optionalString stdenv.hostPlatform.isLinux "-C link-arg=-fuse-ld=mold"
    + lib.optionalString stdenv.hostPlatform.isDarwin "-C link-arg=-fuse-ld=lld";
  commonArgs = {
    pname = cliPackageJson.name;
    inherit version src;
    cargoVendorDir = craneLib.vendorCargoDeps { inherit src; };
    strictDeps = true;
    doCheck = false;
    installCheckPhase = ''
      runHook preInstallCheck
      test "$("$out/bin/ccusage" --version)" = "ccusage ${version}"
      runHook postInstallCheck
    '';
    cargoExtraArgs = "-p ccusage --bin ccusage";
    CCUSAGE_PRICING_JSON_PATH = "${inputs.litellm}";
    CCUSAGE_VERSION = version;
    RUSTFLAGS =
      # NOTE(2026-11-16, #1251): libc 0.2.191 links -liconv on Apple targets. Strip the unused dylib so the binary runs on Macs without a Nix store.
      lib.optionalString stdenv.hostPlatform.isDarwin "-C link-arg=-Wl,-dead_strip_dylibs "
      + linkerRustflags
      + " -C link-arg=-Wl,--icf=all";
    nativeBuildInputs = [
      pkg-config
    ]
    ++ lib.optionals stdenv.hostPlatform.isLinux [ mold ]
    ++ lib.optionals stdenv.hostPlatform.isDarwin [ lld ];
    buildInputs = lib.optionals stdenv.hostPlatform.isDarwin [
      apple-sdk_15
    ];
  }
  // cargoConfigEnv
  // lib.optionalAttrs (telemetryEndpoint != null) {
    CCUSAGE_TELEMETRY_ENDPOINT = telemetryEndpoint;
  };
  cargoTomlFilter =
    path:
    !lib.lists.hasPrefix [
      "package"
      "version"
    ] path
    && craneLib.filters.cargoTomlConservative path;
  depsOnlyArgs =
    builtins.removeAttrs commonArgs [
      "CCUSAGE_PRICING_JSON_PATH"
      "CCUSAGE_TELEMETRY_ENDPOINT"
      "CCUSAGE_VERSION"
      "src"
    ]
    // {
      version = "0.0.0";
      dummySrc = craneLib.mkDummySrc {
        inherit src;
        cleanCargoTomlFilter = cargoTomlFilter;
      };
    };
  dependencyArtifacts = craneLib.buildDepsOnly depsOnlyArgs;
  workspaceArtifacts = import ./nix/cargo-artifacts.nix {
    inherit
      commonArgs
      craneLib
      lib
      pkgs
      root
      ;
    cargoArtifacts = dependencyArtifacts;
    inherit (depsOnlyArgs) dummySrc;
  };
  cargoArtifacts = workspaceArtifacts.adapters;
  unsafeCodeCheck = import ./nix/unsafe-code-check.nix {
    inherit
      commonArgs
      craneLib
      lib
      nushell
      root
      ;
  };
  systemDylibsGate = ''
    if otool -L "$out/bin/ccusage" | awk '/^\t/ { print $1 }' | grep -Ev '^(/usr/lib/|/System/Library/)'; then
      echo "error: $out/bin/ccusage links dylibs that do not exist on end-user machines" >&2
      exit 1
    fi
  '';
in
craneLib.buildPackage (
  commonArgs
  // {
    inherit cargoArtifacts;
    doInstallCheck = true;
    postInstall = lib.optionalString stdenv.hostPlatform.isDarwin systemDylibsGate;
    passthru = {
      inherit
        cargoArtifacts
        commonArgs
        dependencyArtifacts
        depsOnlyArgs
        systemDylibsGate
        linkerRustflags
        unsafeCodeCheck
        version
        workspaceArtifacts
        ;
    };
    meta = {
      inherit (cliPackageJson) description homepage;
      license = lib.getLicenseFromSpdxId cliPackageJson.license;
      mainProgram = builtins.head (builtins.attrNames cliPackageJson.bin);
    };
  }
)
