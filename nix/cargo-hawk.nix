{
  fetchurl,
  lib,
  stdenv,
  stdenvNoCC,
  autoPatchelfHook,
  makeWrapper,
  rust-bin,
  frontendRustflags,
}:
let
  version = "0.1.18";
  # NOTE(2026-11-25, #2119): Hawk links rustc_private. Its archive needs the exact stable Rust it was built against, not the workspace's nightly.
  rustToolchain = rust-bin.stable."1.100.0".minimal;
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
stdenvNoCC.mkDerivation {
  pname = "cargo-hawk";
  inherit version;

  src = fetchurl {
    url = "https://github.com/astral-sh/hawk/releases/download/${version}/cargo-hawk-${archive.target}.tar.gz";
    inherit (archive) hash;
  };

  sourceRoot = "cargo-hawk-${archive.target}";

  nativeBuildInputs = [
    makeWrapper
  ]
  ++ lib.optionals stdenv.hostPlatform.isLinux [ autoPatchelfHook ];
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
    ''}
    # NOTE(2026-11-25): Stable rustc rejects the nightly-only frontend flags, and Cargo prefers RUSTFLAGS over build.rustflags, so always export RUSTFLAGS without them.
    wrapProgram "$out/bin/cargo-hawk" \
      ${lib.optionalString stdenv.hostPlatform.isDarwin ''--prefix PATH : "$out/libexec/hawk-tools" ''}\
      --prefix PATH : ${rustToolchain}/bin \
      --set CARGO ${rustToolchain}/bin/cargo \
      --set RUSTC ${rustToolchain}/bin/rustc \
      --set RUSTDOC ${rustToolchain}/bin/rustdoc \
      --run ${lib.escapeShellArg ''export RUSTFLAGS="''${RUSTFLAGS//${frontendRustflags}/}"''}
    runHook postInstall
  '';

  passthru = { inherit rustToolchain; };

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
