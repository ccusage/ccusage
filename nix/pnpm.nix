{ lib, ... }:
{
  perSystem =
    { config, pkgs, ... }:
    let
      expected = "pnpm@${config.packages.pnpm.version}";
      syncPackageManager = pkgs.writeShellApplication {
        name = "sync-pnpm-package-manager";
        runtimeInputs = [ pkgs.jq ];
        text = ''
          for manifest in "$@"; do
            if [[ "$(jq -r '.packageManager' "$manifest")" != '${expected}' ]]; then
              tmp="$(mktemp)"
              jq --tab --arg expected '${expected}' '.packageManager = $expected' "$manifest" > "$tmp"
              mv "$tmp" "$manifest"
            fi
          done
        '';
      };
    in
    {
      packages.pnpm = pkgs.pnpm_12;
      treefmt.settings.formatter.pnpm-package-manager = {
        command = lib.getExe syncPackageManager;
        includes = [ "package.json" ];
        priority = 0;
      };
    };
}
