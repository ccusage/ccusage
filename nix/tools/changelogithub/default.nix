# Nix-built `changelogithub` CLI, run by the release workflow to publish the
# GitHub release notes for a tag.
#
# It shells out to git (it resolves the release tag with
# `git tag --points-at HEAD` and reads the log between tags), so git is wired
# into the wrapper rather than left to whatever the runner happens to provide.
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
