{
  flake,
  system,
  package,
}:
let
  shipped = (builtins.getFlake flake).packages.${system}.${package};
in
{
  inherit shipped;
  # NOTE(2026-11-21): The strip override is scoped to the `ccusage` package. A profile-wide one changes every dependency fingerprint and rebuilds the cached cargo artifacts.
  symbols = shipped.overrideAttrs (old: {
    pname = "${old.pname}-symbols";
    dontStrip = true;
    preBuild = (old.preBuild or "") + ''
      printf '\n[profile.release.package.ccusage]\nstrip = "none"\n' >> "$CARGO_HOME/config.toml"
    '';
  });
}
