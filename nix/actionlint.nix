# NOTE(2026-11-17): Actionlint 1.7.12 can deadlock before starting ShellCheck on Darwin (rhysd/actionlint#650). Patch it until nixpkgs includes fd33e9f5.
{ actionlint }:
actionlint.overrideAttrs (old: {
  patches = (old.patches or [ ]) ++ [ ./patches/actionlint-stdin.patch ];
  postBuild = (old.postBuild or "") + ''
    go test -run '^TestCmdExecutionLargeStdin$' -timeout 15s .
  '';
})
