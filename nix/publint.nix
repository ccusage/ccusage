{
  fetchPnpmDeps,
  lib,
  makeWrapper,
  nodejs,
  pnpmConfigHook,
  pnpm_11,
  runCommand,
  stdenvNoCC,
}:
let
  pnpm = pnpm_11.override { inherit nodejs; };
  src = runCommand "publint-pnpm-src" { } ''
    mkdir -p "$out"
    cat > "$out/package.json" <<'EOF'
    {"name":"publint-nix","version":"0.0.0","private":true,"devDependencies":{"publint":"0.3.12"}}
    EOF
    cat > "$out/pnpm-lock.yaml" <<'EOF'
    lockfileVersion: '9.0'

    settings:
      autoInstallPeers: true
      excludeLinksFromLockfile: false

    importers:

      .:
        devDependencies:
          publint:
            specifier: 0.3.12
            version: 0.3.12

    packages:

      '@publint/pack@0.1.4':
        resolution: {integrity: sha512-HDVTWq3H0uTXiU0eeSQntcVUTPP3GamzeXI41+x7uU9J65JgWQh3qWZHblR1i0npXfFtF+mxBiU2nJH8znxWnQ==}
        engines: {node: '>=18'}

      mri@1.2.0:
        resolution: {integrity: sha512-tzzskb3bG8LvYGFF/mDTpq3jpI6Q9wc3LEmBaghu+DdCssd1FakN7Bc0hVNmEyGq1bq3RgfkCb3cmQLpNPOroA==}
        engines: {node: '>=4'}

      package-manager-detector@1.6.0:
        resolution: {integrity: sha512-61A5ThoTiDG/C8s8UMZwSorAGwMJ0ERVGj2OjoW5pAalsNOg15+iQiPzrLJ4jhZ1HJzmC2PIHT2oEiH3R5fzNA==}

      picocolors@1.1.1:
        resolution: {integrity: sha512-xceH2snhtb5M9liqDsmEw56le376mTZkEX/jEb/RxNFyegNul7eNslCXP9FDj/Lcu0X8KEyMceP2ntpaHrDEVA==}

      publint@0.3.12:
        resolution: {integrity: sha512-1w3MMtL9iotBjm1mmXtG3Nk06wnq9UhGNRpQ2j6n1Zq7YAD6gnxMMZMIxlRPAydVjVbjSm+n0lhwqsD1m4LD5w==}
        engines: {node: '>=18'}
        hasBin: true

      sade@1.8.1:
        resolution: {integrity: sha512-xal3CZX1Xlo/k4ApwCFrHVACi9fBqJ7V+mwhBsuf/1IOKbBy098Fex+Wa/5QMubw09pSZ/u8EY8PWgevJsXp1A==}
        engines: {node: '>=6'}

    snapshots:

      '@publint/pack@0.1.4': {}

      mri@1.2.0: {}

      package-manager-detector@1.6.0: {}

      picocolors@1.1.1: {}

      publint@0.3.12:
        dependencies:
          '@publint/pack': 0.1.4
          package-manager-detector: 1.6.0
          picocolors: 1.1.1
          sade: 1.8.1

      sade@1.8.1:
        dependencies:
          mri: 1.2.0
    EOF
  '';
in
stdenvNoCC.mkDerivation (finalAttrs: {
  pname = "publint";
  version = "0.3.12";
  inherit src;

  nativeBuildInputs = [
    makeWrapper
    nodejs
    pnpm
    pnpmConfigHook
  ];

  pnpmDeps = fetchPnpmDeps {
    inherit (finalAttrs)
      pname
      version
      src
      ;
    inherit pnpm;
    fetcherVersion = 4;
    hash = "sha256-/XJ8DP7884qt1ZrVpJdixIizanQPLUaYyCucwt/SzXc=";
  };

  dontBuild = true;

  installPhase = ''
    runHook preInstall

    toolRoot="$out/lib/publint"
    mkdir -p "$toolRoot" "$out/bin"
    cp -R node_modules "$toolRoot/node_modules"
    makeWrapper "$toolRoot/node_modules/.bin/publint" "$out/bin/publint" \
      --prefix PATH : ${lib.makeBinPath [ nodejs ]}

    runHook postInstall
  '';

  meta = {
    description = "Lint packaging errors";
    homepage = "https://publint.dev";
    license = lib.licenses.mit;
    mainProgram = "publint";
    platforms = lib.platforms.all;
  };
})
