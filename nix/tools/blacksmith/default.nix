{
  lib,
  stdenvNoCC,
  fetchurl,
  makeWrapper,
  openssh,
  rsync,
}:
let
  version = "0.5.3";
  sources = {
    aarch64-darwin = {
      path = "darwin/arm64";
      sha256 = "51a13e1e2f44aa79c9bbce5bbee849ef749aea95cb1201ef3fea3108520133f8";
    };
    x86_64-darwin = {
      path = "darwin/amd64";
      sha256 = "8b62cd57b74202d87549e8ffbaa0ae5e51add18b6ca97c019b04783fb9bab6fc";
    };
    x86_64-linux = {
      path = "linux/amd64";
      sha256 = "089a301ea5cc93a7560c7f1106911547fdfd46fc5c0cef7079e2a62a22e971ce";
    };
    aarch64-linux = {
      path = "linux/arm64";
      sha256 = "4acca15cb108b2ccff295dfd3e444b81a90fd523d5744c65fc52a3ecbbce7d17";
    };
  };
  source =
    sources.${stdenvNoCC.hostPlatform.system}
      or (throw "blacksmith: unsupported system ${stdenvNoCC.hostPlatform.system}");
in
stdenvNoCC.mkDerivation {
  pname = "blacksmith";
  inherit version;

  src = fetchurl {
    url = "https://clireleases.blacksmith.sh/cli/v${version}/${source.path}/blacksmith";
    inherit (source) sha256;
  };

  dontUnpack = true;
  nativeBuildInputs = [ makeWrapper ];

  installPhase = ''
    runHook preInstall
    install -Dm755 $src $out/libexec/blacksmith
    makeWrapper $out/libexec/blacksmith $out/bin/blacksmith \
      --set BLACKSMITH_DISABLE_AUTO_UPDATE 1 \
      --prefix PATH : ${
        lib.makeBinPath [
          rsync
          openssh
        ]
      }
    runHook postInstall
  '';

  meta = {
    description = "Blacksmith CLI for testboxes";
    homepage = "https://docs.blacksmith.sh";
    mainProgram = "blacksmith";
    platforms = builtins.attrNames sources;
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
  };
}
