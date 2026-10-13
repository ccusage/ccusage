{ inputs, ... }:
{
  perSystem =
    { system, ... }:
    {
      # Unmodified so it substitutes from cache.nixos.org; autoDedupe needs >= 12.8.1.
      packages.pnpm = inputs.pnpm-nixpkgs.legacyPackages.${system}.pnpm_12;
    };
}
