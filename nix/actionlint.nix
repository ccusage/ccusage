# NOTE(2026-11-17, actionlint 1.7.12, rhysd/actionlint#650): shellcheck runs
# deadlock on darwin; upstream fixed it in fd33e9f5 but has not released it.
# Drop this override once nixpkgs ships a release containing that commit.
{ actionlint }:
actionlint.overrideAttrs (old: {
  patches = (old.patches or [ ]) ++ [ ./patches/actionlint-stdin.patch ];
  postBuild = (old.postBuild or "") + ''
    go test -run '^TestCmdExecutionLargeStdin$' -timeout 15s .
  '';
})
