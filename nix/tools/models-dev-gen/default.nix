{
  pkgs,
  bunNodeModules,
  modelsDevSrc,
}:
let
  nodeModules = bunNodeModules { toolDir = ./.; };
in
pkgs.runCommand "models-dev-snapshots"
  {
    nativeBuildInputs = [ pkgs.bun ];
  }
  ''
    export HOME="$TMPDIR"

    cp -r ${modelsDevSrc} work
    chmod -R u+w work

    cp -R ${nodeModules}/node_modules work/node_modules
    chmod -R u+w work/node_modules

    cp ${./gen.ts} work/gen.ts
    cp ${./compact.ts} work/compact.ts
    cp ${./catalog.ts} work/catalog.ts

    cd work
    snapshots="$out/rust/crates/ccusage-pricing-snapshots/src"
    mkdir -p "$snapshots"
    OUTFILE="$snapshots/models-dev-pricing.json" \
      CATALOG_RULES_OUTFILE="$snapshots/models-dev-catalog-rules.json" \
      bun run gen.ts
  ''
