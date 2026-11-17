{
  craneLib,
  inputs,
  lib,
  lld,
  mold,
  pkgs,
  pkg-config,
  root ? ./.,
  stdenv,
  apple-sdk_15,
}:
let
  # The root manifest is the version source of truth: rust/crates/ccusage/build.rs
  # falls back to reading it when CCUSAGE_VERSION is unset, and a test in main.rs
  # asserts the compiled version matches it.
  inherit (lib.importJSON (root + /package.json)) version;
  # The published npm manifest owns the name and the user-facing metadata, so
  # `meta` below derives from it instead of restating it in Nix.
  cliPackageJson = lib.importJSON (root + /apps/ccusage/package.json);
  src = lib.cleanSourceWith {
    src = root + /rust;
    filter =
      path: type:
      (craneLib.filterCargoSources path type)
      || lib.hasSuffix "/cli-help.json" path
      || lib.hasSuffix "/cli-commands.json" path
      || lib.hasSuffix "/fast-multiplier-overrides.json" path
      || lib.hasSuffix "/flex-multiplier-overrides.json" path
      || lib.hasSuffix "/ultrafast-multiplier-overrides.json" path
      || (
        dirOf path == toString (root + /rust/crates/ccusage-pricing-snapshots/src)
        && lib.hasPrefix "models-dev-" (baseNameOf path)
        && lib.hasSuffix ".json" path
      );
  };
  # sqlite3-src's build script turns every SQLITE_* environment variable into a -D
  # define for the bundled amalgamation, which is how .cargo/config.toml trims it.
  # Cargo only reads that file when the working directory is the repository root, and
  # no Nix build has that, so the same values are passed through here rather than
  # duplicated.
  cargoConfigEnv = (builtins.fromTOML (builtins.readFile (root + /.cargo/config.toml))).env;
  linkerRustflags =
    lib.optionalString stdenv.hostPlatform.isLinux "-C link-arg=-fuse-ld=mold"
    + lib.optionalString stdenv.hostPlatform.isDarwin "-C link-arg=-fuse-ld=lld";
  commonArgs = {
    pname = cliPackageJson.name;
    inherit version src;
    # Vendoring from mkDummySrc would be import-from-derivation.
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
      # NOTE(2026-11-17, libc 0.2.191, #1251): libc links -liconv on Apple
      # targets; ccusage uses no iconv symbols, and a /nix/store libiconv
      # crashes non-Nix Macs, so dead-strip unused dylibs.
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
  // cargoConfigEnv;
  # Keep the dependency artifact keyed only by inputs that affect Cargo deps.
  # Pricing snapshots and the npm release version are embedded by the final
  # package. The Rust workspace packages intentionally stay at 0.0.0, and the
  # dummy manifest filter makes that version metadata irrelevant to dependency
  # resolution too.
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
  # End-user Macs have no /nix/store, so fail the build if the binary links
  # anything outside the macOS system paths.
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
