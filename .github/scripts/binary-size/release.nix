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
  symbols = shipped.overrideAttrs (
    old:
    {
      pname = "${old.pname}-symbols";
      dontStrip = true;
    }
    // (
      # NOTE(2026-11-21, #2457): A profile override changes the crate's symbol hashes, so a PGO build would lose its profile. RUSTFLAGS keep the hashes, at the cost of rebuilding the dependencies.
      if old.passthru ? pgoProfile then
        { RUSTFLAGS = "${old.RUSTFLAGS} -Cstrip=none"; }
      else
        {
          # NOTE(2026-11-21): The strip override is scoped to the `ccusage` package. A profile-wide one changes every dependency fingerprint and rebuilds the cached cargo artifacts.
          preBuild = (old.preBuild or "") + ''
            printf '\n[profile.release.package.ccusage]\nstrip = "none"\n' >> "$CARGO_HOME/config.toml"
          '';
        }
    )
  );
}
