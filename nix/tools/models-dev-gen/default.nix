# Reproducibly regenerate compacted models.dev snapshots from the pinned
# `models-dev` flake input. The upstream repository ships per-model TOML sources
# rather than a prebuilt catalog, so we run their own `generateCatalog` routine
# (via the sibling `gen.ts`) with Bun. Its only runtime dependencies are `remeda`
# and `zod`, resolved by bun2nix from the `package.json` next to this file.
#
# The build outputs are copied into the repository (see
# `just gen-models-dev-pricing`) and embedded at build time, so every platform
# ships identical pinned data without build-time network access.
{
  pkgs,
  bunNodeModules,
  modelsDevSrc,
}:
let
  # Keep the versions in the sibling `package.json` matching the `remeda` and
  # `zod` entries in `${modelsDevSrc}/bun.lock` whenever the input is bumped,
  # then run `just gen-bun-nix`.
  nodeModules = bunNodeModules { toolDir = ./.; };
in
pkgs.runCommand "models-dev-snapshots"
  {
    nativeBuildInputs = [ pkgs.bun ];
  }
  ''
    export HOME="$TMPDIR"

    # Copy the read-only source tree into a writable workspace so that Bun can
    # resolve the vendored `node_modules` while importing upstream modules.
    cp -r ${modelsDevSrc} work
    chmod -R u+w work

    # Place the runtime dependencies at the workspace root; Bun walks up from
    # each source file to find them.
    cp -R ${nodeModules}/node_modules work/node_modules
    chmod -R u+w work/node_modules

    # The generator imports `./packages/core/src/generate.ts`, so it must run
    # from inside the workspace next to the vendored node_modules. It also
    # imports its sibling `./compact.ts`, so copy that alongside it under the
    # same relative name the import expects.
    cp ${./gen.ts} work/gen.ts
    cp ${./compact.ts} work/compact.ts

    cd work
    mkdir -p "$out"
    OUTFILE="$out/models-dev-pricing.json" \
      CODEX_AUTO_REVIEW_FALLBACKS_OUTFILE="$out/codex-auto-review-fallbacks.json" \
      bun run gen.ts
  ''
