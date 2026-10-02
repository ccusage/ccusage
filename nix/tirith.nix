{ inputs, ... }:
{
  perSystem =
    { system, ... }:
    {
      packages.tirith = inputs.tirith-nixpkgs.legacyPackages.${system}.tirith;
    };
}
