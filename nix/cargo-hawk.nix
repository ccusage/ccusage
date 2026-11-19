{
  fetchurl,
  lib,
  stdenv,
  stdenvNoCC,
  autoPatchelfHook,
  makeWrapper,
  rustToolchain,
}:
let
  version = "0.1.18";
  # NOTE(2026-11-16, #2119): Hawk links rustc_private. Its archive needs the exact Rust channel it was built against.
  supportedRustChannel = "1.100.0";
  toolchain = builtins.fromTOML (builtins.readFile ../rust-toolchain.toml);
  archives = {
    aarch64-darwin = {
      target = "aarch64-apple-darwin";
      hash = "sha256-lrqfFQOhlOuHE/AG3yhugqCSmfP9c+su/5jMPODUzYI=";
    };
    aarch64-linux = {
      target = "aarch64-unknown-linux-gnu";
      hash = "sha256-sHjCULP3QrEQupwskGQI3RmNCKSn6AXszFuDtvoKHSI=";
    };
    x86_64-linux = {
      target = "x86_64-unknown-linux-gnu";
      hash = "sha256-hOK2WoVx0KqoT+H+oVSr2/iO//9k5ZqJCr+x1VSteKE=";
    };
  };
  archive =
    archives.${stdenv.hostPlatform.system}
      or (throw "cargo-hawk has no release for ${stdenv.hostPlatform.system}");
in
assert lib.assertMsg (toolchain.toolchain.channel == supportedRustChannel)
  "cargo-hawk ${version} requires Rust ${supportedRustChannel}, but rust-toolchain.toml pins ${toolchain.toolchain.channel}";
stdenvNoCC.mkDerivation {
  pname = "cargo-hawk";
  inherit version;

  src = fetchurl {
    url = "https://github.com/astral-sh/hawk/releases/download/${version}/cargo-hawk-${archive.target}.tar.gz";
    inherit (archive) hash;
  };

  sourceRoot = "cargo-hawk-${archive.target}";

  nativeBuildInputs =
    lib.optionals stdenv.hostPlatform.isLinux [ autoPatchelfHook ]
    ++ lib.optionals stdenv.hostPlatform.isDarwin [ makeWrapper ];
  buildInputs = lib.optionals stdenv.hostPlatform.isLinux [
    stdenv.cc.cc.lib
    rustToolchain
  ];

  installPhase = ''
    runHook preInstall
    mkdir -p "$out/bin"
    install -m555 cargo-hawk cargo-hawk-driver cargo-hawk-rustdoc "$out/bin/"
    ${lib.optionalString stdenv.hostPlatform.isDarwin ''
      mkdir -p "$out/libexec/hawk-tools"
      for tool in cc c++ clang clang++; do
        makeWrapper "${stdenv.cc}/bin/$tool" "$out/libexec/hawk-tools/$tool" \
          --unset DYLD_LIBRARY_PATH --unset DYLD_FALLBACK_LIBRARY_PATH
      done
      for tool in ar ranlib; do
        makeWrapper "${stdenv.cc.bintools}/bin/$tool" "$out/libexec/hawk-tools/$tool" \
          --unset DYLD_LIBRARY_PATH --unset DYLD_FALLBACK_LIBRARY_PATH
      done
      wrapProgram "$out/bin/cargo-hawk" \
        --prefix PATH : "$out/libexec/hawk-tools"
    ''}
    runHook postInstall
  '';

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
