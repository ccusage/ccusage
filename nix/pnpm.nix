_: {
  perSystem =
    { config, pkgs, ... }:
    let
      manifest = builtins.fromJSON (builtins.readFile ../package.json);
      expected = "pnpm@${config.packages.pnpm.version}";
    in
    {
      packages.pnpm = pkgs.pnpm_12;
      checks.pnpm-version = pkgs.runCommand "pnpm-version-check" { } ''
        if [ '${manifest.packageManager}' != '${expected}' ]; then
          echo "ERROR: package.json packageManager must be ${expected}, matching the flake." >&2
          exit 1
        fi
        touch "$out"
      '';
    };
}
