{
  bunNodeModules,
  lib,
  makeWrapper,
  nodejs,
  stdenvNoCC,
}:
{
  toolDir,
  pname,
  runtimeInputs ? [ ],
  wrapperArgs ? "",
  meta ? { },
}:
let
  manifest = lib.importJSON (toolDir + "/package.json");
  nodeModules = bunNodeModules { inherit toolDir; };
in
stdenvNoCC.mkDerivation {
  inherit pname;
  inherit (manifest) version;

  dontUnpack = true;

  nativeBuildInputs = [ makeWrapper ];

  dontPatchShebangs = true;

  installPhase = ''
    runHook preInstall

    toolRoot="$out/lib/${pname}"
    mkdir -p "$toolRoot" "$out/bin"
    cp -R ${nodeModules}/node_modules "$toolRoot/node_modules"
    chmod -R u+w "$toolRoot/node_modules"

    grep -rlE '^#!.*bun-with-fake-node/bin/node' "$toolRoot" | while IFS= read -r script; do
      sed -i "1s|^#!.*bun-with-fake-node/bin/node|#!${lib.getExe nodejs}|" "$script"
    done

    makeWrapper ${lib.getExe nodejs} "$out/bin/${pname}" \
      --add-flags "$toolRoot/node_modules/.bin/${pname}" \
      ${wrapperArgs} \
      ${lib.optionalString (runtimeInputs != [ ]) ''
        --prefix PATH : ${lib.makeBinPath runtimeInputs}
      ''}

    runHook postInstall
  '';

  meta = {
    mainProgram = pname;
    platforms = lib.platforms.all;
  }
  // meta;
}
