{
  craneLib,
  inputs,
  lib,
  mold,
  pkg-config,
  root ? ./.,
  stdenv,
  apple-sdk_15,
}:
let
  inherit ((builtins.fromJSON (builtins.readFile (root + /package.json)))) version;
  src = lib.cleanSourceWith {
    src = root + /rust;
    filter =
      path: type:
      (craneLib.filterCargoSources path type)
      || lib.hasSuffix "/cli-help.json" path
      || lib.hasSuffix "/cli-commands.json" path
      || lib.hasSuffix "/fast-multiplier-overrides.json" path
      || lib.hasSuffix "/models-dev-pricing.json" path
      || lib.hasSuffix "/codex-auto-review-fallbacks.json" path;
  };
  commonArgs = {
    pname = "ccusage";
    inherit version src;
    strictDeps = true;
    doCheck = false;
    cargoExtraArgs = "-p ccusage --bin ccusage";
    CCUSAGE_PRICING_JSON_PATH = "${inputs.litellm}/model_prices_and_context_window.json";
    CCUSAGE_VERSION = version;
    RUSTFLAGS =
      lib.optionalString stdenv.isLinux "-C link-arg=-fuse-ld=mold"
      # The nixpkgs Darwin stdenv injects -liconv even though ccusage uses no
      # iconv symbols, recording an unused /nix/store libiconv dependency that
      # crashes non-Nix Macs (#1251). dead_strip_dylibs drops dylib load
      # commands with no referenced symbols, so the unused libiconv is removed
      # and the binary links only system dylibs.
      + lib.optionalString stdenv.isDarwin "-C link-arg=-Wl,-dead_strip_dylibs";
    nativeBuildInputs = [
      pkg-config
    ]
    ++ lib.optionals stdenv.isLinux [ mold ];
    buildInputs = lib.optionals stdenv.isDarwin [
      apple-sdk_15
    ];
  };
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
  cargoArtifacts = craneLib.buildDepsOnly depsOnlyArgs;
in
craneLib.buildPackage (
  commonArgs
  // {
    inherit cargoArtifacts;
    postInstall = lib.optionalString stdenv.isDarwin ''
      # The nixpkgs Darwin stdenv cc-wrapper injects -liconv into the link even
      # when libiconv is absent from buildInputs.  dead_strip_dylibs (set in
      # RUSTFLAGS) drops it locally, but in CI the cc-wrapper's -liconv may
      # arrive after the linker has resolved dead_strip_dylibs.  Rewrite the
      # install name as a robust fallback so the safety gate below doesn't fail
      # on a dylib that carries no referenced symbols.
      for lib in $(otool -L "$out/bin/ccusage" | tail -n +2 | awk '{print $1}' | grep -E '^/nix/store/[^/]+-libiconv-'); do
        install_name_tool -change "$lib" /usr/lib/libiconv.2.dylib "$out/bin/ccusage"
      done
      # Every remaining dylib MUST be a macOS system path.  grep prints the
      # offending entries when it matches — fail the build for any matches.
      if otool -L "$out/bin/ccusage" | tail -n +2 | awk '{print $1}' | grep -Ev '^(/usr/lib/|/System/Library/)'; then
        echo "error: ccusage links dylibs that do not exist on end-user machines" >&2
        exit 1
      fi
    '';
    passthru = {
      inherit
        cargoArtifacts
        commonArgs
        depsOnlyArgs
        version
        ;
    };
    meta = {
      description = "Analyze coding agent CLI token usage and costs from local data";
      homepage = "https://github.com/ccusage/ccusage";
      license = lib.licenses.mit;
      mainProgram = "ccusage";
    };
  }
)
