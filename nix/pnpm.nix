{ inputs, ... }:
{
  perSystem =
    { system, ... }:
    {
      # Use nixpkgs' build unmodified so it substitutes from cache.nixos.org.
      # Overriding the version forces a from-source Rust build, which takes
      # ~25 minutes on the GitHub-hosted runner that publishes to npm. It must
      # stay at 12.8.1 or newer for pnpm-workspace.yaml's autoDedupe.
      packages.pnpm = inputs.pnpm-nixpkgs.legacyPackages.${system}.pnpm_12;
    };
}
