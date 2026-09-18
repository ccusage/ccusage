{ inputs, ... }:
{
  perSystem =
    { system, ... }:
    {
      packages.pnpm = inputs.pnpm-nixpkgs.legacyPackages.${system}.pnpm_12;
    };
}
