# hawk, a workspace-aware Cargo lint for unnecessary Rust visibility.
#
# The upstream release is used rather than a source build because hawk links
# against `rustc_private` and would otherwise need the `rustc-dev` component and
# `RUSTC_BOOTSTRAP=1`. A prebuilt binary carries the same constraint in a simpler
# form: it only runs on the compiler it was built against, which is why
# rust-toolchain.toml pins the version in `supportedRustChannel` below.
{
  fetchurl,
  lib,
  stdenv,
  stdenvNoCC,
  autoPatchelfHook,
}:
let
  version = "0.1.10";
  supportedRustChannel = "1.97.1";
  # sha256 values come from the `.sha256` file published beside each archive.
  archives = {
    aarch64-darwin = {
      target = "aarch64-apple-darwin";
      hash = "sha256-Ie8mN4WMH5KncdCacAtOl3r6fO8/qjxSzkgyK4noHCM=";
    };
    x86_64-darwin = {
      target = "x86_64-apple-darwin";
      hash = "sha256-zMvrswFj/XKHMprRhGzsuAoUs8lqp2agLmhJwuynizE=";
    };
    aarch64-linux = {
      target = "aarch64-unknown-linux-gnu";
      hash = "sha256-jsuPCOKgktU3DMsZleR5d/akV/rxI3zOdhmvV1YPe3I=";
    };
    x86_64-linux = {
      target = "x86_64-unknown-linux-gnu";
      hash = "sha256-9uwbGG0oZMQv86Atcwl0Nzy1x7ULl6BpFdgQ2cIcsjg=";
    };
  };
  archive =
    archives.${stdenv.hostPlatform.system}
      or (throw "cargo-hawk has no release for ${stdenv.hostPlatform.system}");
in
stdenvNoCC.mkDerivation {
  pname = "cargo-hawk";
  inherit version;

  src = fetchurl {
    url = "https://github.com/astral-sh/hawk/releases/download/${version}/cargo-hawk-${archive.target}.tar.gz";
    inherit (archive) hash;
  };

  # The archive holds a single cargo-hawk-<target>/ directory.
  sourceRoot = "cargo-hawk-${archive.target}";

  # The prebuilt Linux archives are ordinary glibc binaries, so they need the
  # loader and rpath fixups; the Darwin ones run as shipped.
  nativeBuildInputs = lib.optionals stdenv.hostPlatform.isLinux [ autoPatchelfHook ];

  installPhase = ''
    runHook preInstall
    mkdir -p "$out/bin"
    # cargo-hawk execs cargo-hawk-driver from its own directory, so both have to
    # land together.
    install -m555 cargo-hawk cargo-hawk-driver "$out/bin/"
    runHook postInstall
  '';

  passthru = { inherit supportedRustChannel; };

  meta = {
    description = "Workspace-aware Cargo lint for unnecessary public Rust APIs";
    homepage = "https://github.com/astral-sh/hawk";
    license = with lib.licenses; [
      asl20
      mit
    ];
    mainProgram = "cargo-hawk";
    platforms = builtins.attrNames archives;
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
  };
}
