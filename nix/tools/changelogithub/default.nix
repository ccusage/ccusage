{
  bunCli,
  git,
  lib,
}:
bunCli {
  toolDir = ./.;
  pname = "changelogithub";
  runtimeInputs = [ git ];
  meta = {
    description = "Generate changelog for GitHub releases";
    homepage = "https://github.com/antfu/changelogithub";
    license = lib.licenses.mit;
  };
}
