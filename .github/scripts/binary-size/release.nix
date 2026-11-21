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
  # Same derivation and cargo artifacts as the shipped binary. Only the final `ccusage` link keeps
  # its symbol table, so the dependency fingerprints still match and nothing else rebuilds.
  symbols = shipped.overrideAttrs (old: {
    pname = "${old.pname}-symbols";
    dontStrip = true;
    preBuild = (old.preBuild or "") + ''
      printf '\n[profile.release.package.ccusage]\nstrip = "none"\n' >> "$CARGO_HOME/config.toml"
    '';
  });
}
