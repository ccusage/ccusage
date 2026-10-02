{ inputs, ... }:
{
  perSystem =
    { system, ... }:
    {
      # The pinned nixpkgs pnpm predates autoDedupe support.
      packages.pnpm =
        (inputs.pnpm-nixpkgs.legacyPackages.${system}.pnpm_12.override {
          version = "12.8.1";
          srcHash = "sha256-masnC/BOw8SGlwCxbnfHl0qiOatiXacEPYvV6hVyVAY=";
          cargoHash = "sha256-rT3kFHLPVSwAqM2e0HXfeF2rGG/btFr3lsokVhs9OIk=";
        }).overrideAttrs
          (previous: {
            postPatch = (previous.postPatch or "") + ''
              # Nix supplies vendored Cargo sources; pnpm's duplicates the Git source.
              sed -i '/# >>> pnpm-managed cargo sources >>>/,/# <<< pnpm-managed cargo sources <<</d' .cargo/config.toml
            '';
          });
    };
}
